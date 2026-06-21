# Entity Application Framework — Architectural Review

Status: Active spec. Phases 0 and 3 complete;
the AppHandler subscription bridge described below is the next
near-term step and corresponds to Level 4a (Subscribe + Notify) in
the SDK abstraction levels.

This document defines how the entity application framework layer
emerges from patterns in this project, the Go workbench, and the
Godot bindings. The framework sits between the entity-core kernel
and applications, providing protocol-correct access and event bridges.

Naming alignment: the per-peer wrapper described here as `AppPeer`
is now called `PeerContext` in the SDK design. See ENTITY-SDK-API.md
for the full SDK layering (Layer 0 algorithm foundation, Layer 1
protocol facade, Layer 2 algorithm library / shared patterns) and
the five abstraction levels (raw protocol → SDK facade → typed
builders → fluent pipelines → reactive bindings → entity-native
programs).

## Context

Three Rust-consuming projects exist in the entity ecosystem:

| Project | Peer integration | UI | Peer management |
|---------|-----------------|-----|-----------------|
| **entity-browser-rust** (this) | entity-peer direct | DOM | PeerManager (272 LOC, Rust-side state) |
| **godot-entity-core-rust** | GDExtension binding | Godot scenes | GDScript AppState (signal forwarding) |
| **entity-core-rust** (kernel) | IS the peer | N/A | N/A |

The Go workbench has independently converged on a three-layer model:

```
Applications     (console, canvas, future web)
    |
App Framework    (workbench/ → becoming entity-app-go)
    |
Kernel           (entity-core-go/core)
```

The Rust side has the kernel and application layers but no
explicit framework layer. The question is what belongs in that
middle layer and how it relates to what we've already built.

## The Go Team's Framework (workbench/)

10 source files, 43 tests. Key components:

- **Executor** — wraps handler registry, content store, location
  index as unexported fields. Applications interact through
  protocol operations only. `Execute()`, `TreeGet()`, `TreeList()`,
  `TreePut()`.

- **PeerContext** — wraps Executor with caching. Sorted entry list
  with dirty tracking. `Entries()`, `Resolve()`, `MarkDirty()`,
  `RefreshIfDirty()`.

- **DataContext** — interface that both console and canvas renderers
  implement. The contract between data and presentation.

- **Handler discovery** — `DiscoverHandlers(pc)` scans
  `system/handler/*`, returns `[]HandlerInfo` with pattern, name,
  operations. Protocol-level, not store-level.

- **Format pipeline** — renderer-neutral CBOR formatting.
  `FormattedLine` with indent, key, index, value. Classifies
  values (null, bool, string, number, bytes, hash).

- **Selection state** — navigation history, current path, shared
  across renderers.

- **Command registry** — discoverable commands with filtering.

Critical design principle: **Executor holds store and index as
unexported fields.** The application literally cannot bypass the
protocol. This enforces completeness — if the UI can't do
something through the protocol, the protocol is incomplete.

The Go `workbench/` package is partly **Layer 2 territory** in the
SDK layering sense — TreeBrowserModel, DetailModel, FormattedLine,
ValueKind are shared algorithms and patterns that should produce
identical results across implementations. They aren't just Go
ergonomics; they're the projection model that lets a tree view in
console (tview) and a tree view in canvas (raylib) consume the
same logical output. When this project adopts a model layer, those
models will need to align with the Go ones at Layer 2 conformance.

## What entity-peer Currently Exposes

The Rust kernel's public API surface is well-designed but exposes
more than applications should touch:

### Application-facing (stable, use directly)

```rust
// Peer lifecycle
PeerBuilder::new().keypair(k).connector(c).build()
Peer::execute(handler, operation, params) -> HandlerResult
Peer::execute_with_options(handler, operation, params, opts)
Peer::connect_to(addr) -> peer_id
Peer::subscribe_events() -> Receiver<TreeChangeEvent>
Peer::start_engines(&shared)
Peer::shared() -> Arc<PeerShared>

// Handlers
trait Handler { handle, pattern, name, operations }
HandlerContext { operation, params, execute_fn, ... }
HandlerResult::ok(entity)
HandlerRegistry::register(handler)

// Transport
trait Connector { connect(addr) -> Connection }
trait Listener { accept() -> Connection }

// Events
TreeChangeEvent { path, hash, change_type }
```

### Infrastructure (should NOT be in application code)

```rust
// Direct storage — bypasses handlers/capabilities
ContentStore::put(entity), get(hash), remove(hash)
LocationIndex::set(path, hash), get(path), list(prefix)

// Connection internals
RemoteState::get(peer_id), insert(peer_id, conn)
make_execute_fn(shared, author, included)
handle_connection(conn, shared)
send_execute(conn, keypair, uri, ...)

// PeerShared exposes everything as pub fields
PeerShared {
    pub content_store: Arc<dyn ContentStore>,
    pub location_index: Arc<dyn LocationIndex>,
    pub handler_registry: Arc<HandlerRegistry>,
    pub remote: RemoteState,
    ...
}
```

The problem: `PeerShared` has all fields public. This is necessary
for internal use (server, connection handling, engine wiring) but
means applications can — and do — reach into the store directly.

**Resolution**: the SDK makes this boundary
explicit. `PeerContext` exposes L1 dispatched methods as the default
(async, capability-checked) and L0 direct-store access only through
`ctx.store()` — every such call is a visible opt-out. `PeerShared`
fields remain reachable via the `peer()` / `peer_shared()` escape
hatches, but those are named escape hatches, not the happy path.

## Current State of This Project (entity-browser-rust)

### What's framework-like (reusable)

| Component | File | What it does |
|-----------|------|-------------|
| Peers (multi-SDK router) | peers.rs | Routes per-peer ops to the owning SDK (`Direct`/`Worker`); wraps the SDK-tier `PeerManager` (in `entity-sdk`) |
| Action dispatch | app.rs | Routes async execute/connect/listen to the bound peer |
| Format pipeline | format.rs | HandlerResult → display string |
| Tree utilities | via `PeerContext` (entity-sdk) | entity_count, path_count, tree_listing |

### What's application-specific

| Component | File | What it does |
|-----------|------|-------------|
| WindowManager | window.rs | Multi-instance window lifecycle |
| WindowView trait | window.rs | Canvas + DOM rendering per window |
| DomRenderer | dom/mod.rs | Shadow DOM, snapshot rebuild |
| 6 window views | views/*.rs | Entity tree, event log, execute console, etc. |
| Render modes | action.rs | Canvas/Dom/Both/Shadow switching |

### What's missing (the framework gap)

| Capability | Status | Impact |
|-----------|--------|--------|
| Event bridge (wake signal) | **Complete** | `event_bridge()` in sdk.rs, both native and WASM |
| Entity-backed UI state | **Complete** | All windows use tree-backed state |
| Subscription → UI bridge | Not started | No fine-grained reactive updates per path |
| App handler registration | Not started | Can't receive subscription notifications |
| Protocol-only access | Not enforced | PeerManager uses store/index directly |
| Connection lifecycle | Partial | No auto-reconnect, no subscription recovery |
| Type-aware operations | Not started | No reading system/type/* for structural rendering |

## Convergence Spectrum

The Go team frames protocol integration as levels:

| Level | Description | This project | Godot |
|-------|-------------|-------------|-------|
| 0 | Client only (read/execute) | — | — |
| 1 | Cached client | Past this (windows query tree) | Here (signal forwarding) |
| 2 | Handler registration + subscriptions | Partially (engines exist, event bridge done, not wired to subscriptions) | Not started |
| 3 | Full peer with listener | **Here** on native (WS listener) | Here (TCP listener) |
| 4 | Entity-native (all state is entities) | **Partially** (entity-backed window state complete, not yet protocol-only) | Not started |

The execution infrastructure is at Level 3 but the application
architecture is at Level 1. The framework layer bridges this gap.

## The Handler → mpsc Bridge

This is the highest-value missing piece. The plumbing exists in
entity-peer; the bridge does not.

> **Where this fits in the SDK abstraction stack**: the AppHandler
> bridge described here IS Level 4a (Subscribe + Notify) of the SDK
> abstraction levels — the reactive framework via subscriptions and
> typed callbacks. It's not the compute extension (Level 4b); it's
> the simpler, already-shippable reactive layer that uses what
> entity-core-rust already provides. The current implementation
> (event bridge → wake function → DOM snapshot rebuild) is a
> coarse-grained version of the same pattern. AppHandler refines it
> to per-pattern subscription delivery.

### What exists

```
Tree mutation
    → NotifyingLocationIndex fires TreeChangeEvent
    → broadcast::Sender fans out to subscribers
    → Subscription Engine matches patterns
    → DeliverFn calls make_execute_fn
    → Delivers to handler at deliver_uri with "receive" operation
```

All of this works. The subscription extension, inbox handler, and
delivery pipeline are implemented and tested in entity-core-rust.

### What's missing: the AppHandler

```rust
use entity_handler::{Handler, HandlerContext, HandlerResult, HandlerError};
use tokio::sync::mpsc;

/// Application handler that bridges protocol notifications to a channel.
/// Register on the peer at a known URI (e.g., "workspace/app").
/// Subscriptions deliver to this handler's "receive" operation.
pub struct AppHandler {
    tx: mpsc::Sender<Notification>,
}

pub struct Notification {
    pub subscription_id: String,
    pub path: String,
    pub hash: entity_hash::Hash,
    pub change_type: String,
}

impl Handler for AppHandler {
    async fn handle(&self, ctx: &HandlerContext) -> Result<HandlerResult, HandlerError> {
        match ctx.operation.as_str() {
            "receive" => {
                let notif = decode_notification(&ctx.params)?;
                // Non-blocking send — if UI is behind, drop oldest
                let _ = self.tx.try_send(notif);
                Ok(HandlerResult::ok(/* ack entity */))
            }
            _ => Err(HandlerError::NotSupported(ctx.operation.clone())),
        }
    }

    fn pattern(&self) -> &str { "workspace/app" }
    fn name(&self) -> &str { "app" }
    fn operations(&self) -> &[&str] { &["receive"] }
}
```

### UI-side integration

```rust
// During app setup:
let (tx, rx) = mpsc::channel(256);
let app_handler = Arc::new(AppHandler { tx });
peer.handler_registry().register(app_handler);
peer.start_engines(&shared);

// Create subscriptions for paths we care about:
peer.execute("system/subscription", "subscribe", subscribe_params).await?;

// In the render loop (DOM render pass or equivalent):
while let Ok(notif) = self.notification_rx.try_recv() {
    match notif.path.as_str() {
        p if p.starts_with("system/handler/") => { /* refresh handler list */ }
        p if p.starts_with("system/type/") => { /* refresh type info */ }
        _ => { /* mark relevant windows dirty */ }
    }
    // Request repaint since state changed
    ctx.request_repaint();
}
```

### WASM considerations

On WASM there's no tokio runtime. The bridge would use:

```rust
// WASM: no Send requirement
use std::sync::mpsc;  // or a wasm-compatible channel
// spawn_local instead of tokio::spawn
wasm_bindgen_futures::spawn_local(async move { ... });
```

The Handler trait already has WASM-compatible variants (no Send
bound on the Future when target is wasm32). The channel type
needs to differ but the pattern is identical.

## Protocol-Only Access Pattern

The Go Executor pattern adapted for Rust. In the current SDK design,
this is PeerContext — the per-peer handle that hides kernel internals.
See ENTITY-SDK-API.md for the full type definition.

```rust
/// Protocol-only peer access. Hides store/index, all operations
/// go through handler dispatch. This is the framework's primary
/// interface — applications never see ContentStore or LocationIndex.
/// (In SDK nomenclature, this is PeerContext.)
pub struct PeerContext {
    peer: Peer,
    shared: Arc<PeerShared>,  // held internally, not exposed
    notification_rx: mpsc::Receiver<Notification>,
}

impl PeerContext {
    /// Execute a handler operation through the protocol.
    pub async fn execute(
        &self, handler: &str, operation: &str, params: Entity,
    ) -> Result<HandlerResult, HandlerError> {
        self.peer.execute(handler, operation, params).await
    }

    /// L1 dispatched tree get — capability-checked via system/tree handler.
    pub async fn get(&self, path: &str) -> Result<Option<Entity>, SdkError> {
        let params = build_get_params(path);
        let result = self.peer.execute("system/tree", "get", params).await?;
        Ok(extract_entity(&result))
    }

    /// L1 dispatched tree list.
    pub async fn list(&self, prefix: &str) -> Result<Vec<ListingEntry>, SdkError> {
        let params = build_list_params(prefix);
        let result = self.peer.execute("system/tree", "list", params).await?;
        Ok(extract_entries(&result))
    }

    /// L1 dispatched tree put.
    pub async fn put(&self, path: &str, entity: Entity) -> Result<Hash, SdkError> {
        let params = build_put_params(path, entity);
        self.peer.execute("system/tree", "put", params).await?;
        // ...
    }

    /// L0 direct-store access — sync escape hatch. Every call is a visible
    /// opt-out from the security boundary. Use in render loops or
    /// bootstrapping.
    pub fn store(&self) -> StoreAccess<'_> { /* ... */ }

    /// Subscribe to tree changes matching a pattern.
    pub async fn subscribe(&self, pattern: &str) -> Result<String, HandlerError> {
        let params = build_subscribe_params(pattern, "workspace/app", "receive");
        let result = self.peer.execute("system/subscription", "subscribe", params).await?;
        Ok(extract_subscription_id(&result))
    }

    /// Drain pending notifications (call in render loop).
    pub fn drain_notifications(&mut self) -> Vec<Notification> {
        let mut notifs = Vec::new();
        while let Ok(n) = self.notification_rx.try_recv() {
            notifs.push(n);
        }
        notifs
    }

    /// Register a custom application handler.
    pub fn register_handler(&self, handler: Arc<dyn Handler>) {
        self.peer.handler_registry().register(handler);
    }

    /// Peer identity.
    pub fn peer_id(&self) -> &str { self.peer.peer_id().as_str() }
}
```

Note what's exposed: the L1 dispatched methods are the default, and
the sync L0 `store()` escape hatch is explicit (so every bypass is
visible in code review). `content_store()`, `location_index()`,
`shared()` are not on `PeerContext` — reaching them requires the
named escape hatches `peer()` / `peer_shared()`.

### Current sync usage in this project

The app layer (`Peers`) routes reads through the SDK's L0 `store()` handle:

```rust
// peers.rs (app layer) — explicit L0 usage, visible opt-out
pub fn get_entity(&self, peer_id: &str, path: &str) -> Option<Entity> {
    self.sdk_for(peer_id)?.peer_context(peer_id)?.store().get(path)
}

pub fn tree_listing(&self, peer_id: &str, prefix: &str) -> Vec<LocationEntry> {
    self.sdk.peer(peer_id)
        .map(|ctx| ctx.store().list(prefix))
        .unwrap_or_default()
}
```

These are fast (sync, no handler dispatch) but skip capability checks
and emit no events — same trade-off as before, now named. The visible
`store()` call is the security-review signal.

The trade-off: L1 `execute()` is async. In a synchronous render
loop, you can't call it directly. Options:

1. **Cache + background refresh**: Window holds last-known state,
   background task re-queries on notification, updates cache.
2. **Block on small operations**: `futures::executor::block_on()`
   for single tree gets (fast for local peer).
3. **Snapshot model**: Framework maintains a snapshot of subscribed
   paths, updated reactively. Windows read the snapshot (sync).

Option 3 aligns with the DOM snapshot rebuild model we already
use. The framework maintains a reactive cache; windows read it.

## Entity-Backed Application State

Both teams converge on this: UI state lives in the entity tree.

### Tree layout (matching Go's design)

```
{browser-peer-id}/
├── workspace/
│   ├── ui/
│   │   ├── selection              → current path per window
│   │   ├── layout/                → window positions, sizes, open/closed
│   │   ├── mode                   → Canvas/Dom/Both/Shadow
│   │   └── windows/
│   │       ├── 1                  → window instance state
│   │       └── 2
│   ├── settings/
│   │   ├── theme                  → dark/light
│   │   └── default-mode           → startup render mode
│   └── connections/
│       ├── ws://192.168.1.5:4041  → connection state entity
│       └── ws://10.0.0.3:4041
└── system/                        → (exists from peer bootstrap)
    ├── type/
    ├── handler/
    └── ...
```

### What this enables

- **Persistence**: State survives app restarts (entity store can
  be backed by disk/IndexedDB).
- **Inspectability**: Browse your own UI state using the entity
  tree window. Settings window reads `workspace/settings/*`.
- **Portability**: Export workspace state, import on another
  instance. Different renderers (DOM, Godot, and other frontends)
  read the same entity structure.
- **Self-modification**: The browser can browse and modify its
  own configuration through the same protocol.
- **Subscription-driven**: When `workspace/ui/selection` changes,
  subscriptions can notify dependent windows.

### State that moved into the tree (this migration largely landed)

The arc below — moving UI state out of Rust structs and into the entity tree —
is now mostly done (window state, connections, and the event log are all
tree-backed today; there is no `RenderMode` — DOM is the only render path):

```rust
// Was in EntityApp / WindowManager / WindowView → now in the tree:
window_manager.windows: Vec<Instance> → workspace/windows/*
peer_connections state                → connections/*
current_path: String                  → workspace/windows/{id}/state (per-window)
event_log: Vec<String>                → tree-backed event-log writer
```

This is a significant refactor. The Go team hasn't done it yet
either — they've designed it but the workbench still uses Go
structs for most UI state. Both projects are approaching the same
frontier.

## Where Should the Framework Live?

Three options:

### Option A: In entity-core-rust (new crate in core/ or lib/)

```
entity-core-rust/
├── core/
│   ├── peer/          (kernel — exists)
│   ├── store/         (kernel — exists)
│   └── ...
├── lib/
│   └── app/           (framework — new)
│       ├── app_peer.rs
│       ├── app_handler.rs
│       ├── notification.rs
│       └── ...
└── bindings/
    └── godot/         (GDExtension — exists)
```

Pro: Both this app and the Godot project import it. Single source of truth.
Con: entity-core-rust is the kernel; adding application-level
concepts muddies the boundary. The kernel should be unopinionated
about how applications use it.

### Option B: Separate crate (entity-app-rust)

```
entity-systems/
├── entity-core-rust/      (kernel)
├── entity-app-rust/       (framework — new)
├── entity-browser-rust/   (this project, imports framework)
└── godot-entity-core-rust/ (imports framework via binding)
```

Pro: Clean separation. Matches Go's plan (entity-app-go).
Con: Another repo/crate to maintain. May be premature.

### Option C: Extract from this project when ready

Keep building in entity-browser-rust. When the Godot project
needs the same patterns, extract then. The Go team is taking
this path — workbench/ exists in the workbench repo, will become
entity-app-go when it stabilizes.

Pro: No premature abstraction. Build what's needed, extract what's
proven.
Con: Godot may duplicate patterns before extraction happens.

### Recommendation

**Option C now, targeting Option B later.** The framework boundary
isn't clear enough yet to extract cleanly. Both this project and
the Go workbench are still discovering what belongs in the middle
layer. Build the PeerContext, AppHandler, and notification bridge in
this project. When the Godot project picks back up and needs the
same patterns, that's the signal to extract.

The Go team's `workbench/` module at 10 files is a good size
indicator — the framework layer is not large. It's a thin
coordination layer on top of a well-designed kernel.

## Refactoring Path

### Phase 0: Event bridge — COMPLETE

Event bridge implemented in `sdk.rs` as `event_bridge()` with
both native (Send) and WASM (spawn_local) variants. Subscribes
to `peer.subscribe_events()`, calls wake function on each event.
WASM variant also increments generation counter for snapshot
detection. Entity-backed window state also complete — all
windows store state in the peer's tree.

### Phase 1: Cached state (match Go's PeerContext)

Adopt the Go workbench's dirty-flag + cached-entries pattern:

1. PeerContext with `entries: Vec<LocationEntry>`, `dirty: bool`
2. Background event bridge sets dirty flag (already wired in Phase 0)
3. Render loop calls `refresh_if_dirty()` — rebuilds cache from tree
4. Windows read cached entries instead of direct store access

This decouples windows from store locks entirely and is the
natural place for the protocol-only boundary later (cache
rebuilt through `Peer::execute()` instead of direct access).

### Phase 2: Access-level split — COMPLETE

The security boundary is visible in the SDK surface per
`SDK-OPERATIONS.md` §2.7:

- L1 (`ctx.get/put/...` async) routes through `peer.execute(...)`
  — capability-checked, default path.
- L0 (`ctx.store().get/put/...` sync) is the explicit escape hatch.
  Every `store()` call is a visible opt-out from dispatch.

All application code now uses `store()` for direct access (matches
sync render/action loops) or awaits L1 methods. The PeerShared and
store/index internals stay behind the SDK surface. See
ENTITY-SDK-API.md Phase 3 for details.

**Remaining app-side**: make `handle_action` async so view writes can
migrate from L0 to L1. This is an application refactor, not an SDK
change.

### Phase 3: Entity-backed state — MOSTLY COMPLETE

Entity-backed window state is working. All windows store state in
the peer's tree under `{peer_id}/app/entity-browser/workspace/` (per
window) and `/app/entity-browser/settings/` (global), per the SDK
domain namespace convention. Path helpers live in `src/app_paths.rs`
— application layer, not the SDK. Remaining:
- State ownership split between app peer and user peer
  (see PEER-IDENTITY-MODEL.md)
- Migrate writes from L0 (`store().put`) to L1 (`ctx.put().await`)
  once `handle_action` is async

### Phase 4: Subscription engine integration (Level 4a polish)

Wire the full subscription engine with AppHandler when:
- Cross-peer state changes need to trigger UI updates
- Fine-grained per-window notification routing is needed
- Rate limiting matters (high-frequency remote updates)

Until then, the blanket `subscribe_events()` broadcast is
sufficient. This phase corresponds to the path-pattern
subscription work in `ENTITY-SDK-API.md` Phase 4.

### Phase 4.5: Pipeline builder integration (Level 3 / Layer 2)

The Tauri backend peer setup, relay registration, and remote
subscription wiring are all natural pipelines. As the SDK Layer 2
pipeline builder lands (`ENTITY-SDK-API.md` Phase 5), the
framework should expose helpers for the common patterns this app
needs:
- Backend peer setup pipeline
- Remote subscription with resume-on-disconnect
- Bridge construction (knowledge base ↔ file system, etc.)

These are Layer 2 — the framework owns the assembly, the SDK owns
the building blocks.

### Phase 5: Extract to shared crate (when Godot needs it)

1. Factor EntitySDK + PeerContext into entity-sdk-rust
2. Both this app and the Godot project depend on it
3. GDExtension wraps PeerContext instead of raw Peer

## Comparison with Godot's Integration

The Godot GDExtension exposes `EntityPeer` as a Node with methods:
`tree_get`, `tree_put`, `tree_list`, `execute`, `start`, `stop`,
and a `tree_changed` signal.

This is already close to the PeerContext concept — it hides PeerShared
and exposes protocol-level operations. The main differences:

- Godot's EntityPeer calls store/index directly (same bypass as our
  PeerManager) rather than routing through handlers
- No subscription management — just a blanket `tree_changed` signal
- No handler registration API for application handlers
- Signal-based (Godot's event system) rather than channel-based

When the Godot project picks back up, the PeerContext pattern would
replace the current raw store/index calls inside EntityPeer's
implementation, and subscription management would be added to
support targeted updates instead of the current blanket signal.

## Open Questions

1. ~~**Sync vs async in render loops**~~: **Resolved.** The render
   loop reads current state synchronously — it doesn't process
   events or drain channels. State mutations happen on background
   threads. The renderer just needs a wake signal. No sync/async
   tension exists. See `reviews/legacy/FRONTEND-CONCURRENCY.md`
   for the original cross-project analysis.

2. ~~**WASM channel type**~~: **Resolved.** Event bridge implemented
   using `peer.subscribe_events()` broadcast receiver with
   `spawn_local` on WASM. Works without Send bound.

3. **Notification granularity**: Blanket `subscribe_events()` is
   currently sufficient (same as Godot). Finer granularity via
   subscription engine comes later when cross-peer reactive updates
   or per-window filtering is needed.

4. **Store access for performance**: RwLock contention is
   microseconds. Direct store reads are fine for local peer at
   current scale. The Go caching pattern (PeerContext with dirty
   flag) is worth adopting for architectural clarity, not
   performance. Protocol-only access is a boundary decision, not
   a performance decision.

5. ~~**Entity-backed state timing**~~: **Resolved.** Event bridge
   is working, entity-backed window state is implemented. Tree
   mutation → event → repaint → renderer reads new state is the
   active pattern.

## References

- SDK API design: `docs/architecture/specs/ENTITY-SDK-API.md`
- SDK-Peer Boundary analysis (historical): `docs/architecture/reviews/legacy/SDK-PEER-BOUNDARY.md`
- Peer Identity Model: `docs/architecture/specs/PEER-IDENTITY-MODEL.md`
- Go workbench architecture: `entity-workbench-go/docs/architecture/`
  - `ENTITY-APPLICATION-FRAMEWORK.md` — framework layer design
  - `APPLICATION-HANDLER-INTEGRATION.md` — handler bridge + convergence spectrum
  - `WORKBENCH-STATUS-AND-NEXT.md` — build phases
- Entity protocol spec: `entity-core-architecture/docs/architecture/v7.0-core-revision/specs/ENTITY-CORE-PROTOCOL-V7.md`
- This project architecture: `docs/architecture/specs/PROJECT-ARCHITECTURE.md`
- Frontend concurrency (legacy): `docs/architecture/reviews/legacy/FRONTEND-CONCURRENCY.md`
- Godot bindings: `entity-core-rust/bindings/godot/`

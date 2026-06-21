# Entity SDK — Rust API Surface

> **Drift note:** the canonical SDK lives in the
> `entity-sdk` crate at `../entity-core-rust/bindings/sdk/` and is
> consumed by both this app and the Godot binding. SDK changes
> belong upstream; this doc is this app's **consumer view** and
> the bottom third may lag the crate. Canonical references: the
> sdk-domain guides + specs listed at the top of `AGENTS.md`
> (`GUIDE-ENTITY-WORKBENCH-APP`, `GUIDE-PERSISTENCE`,
> `GUIDE-PEER-CONCERNS-AND-NAMESPACES`, `SDK-OPERATIONS`).
> When this doc disagrees with those, the spec wins.

Status: Active spec. Phases 1-2 implemented;
Layer 2 (pipeline builder, content abstraction, bridge framework)
identified as the next major SDK direction.

The Entity SDK is the developer's primary interface for building
entity-native applications in Rust. It manages peers, provides
ergonomic per-peer operations through PeerContext handles, and
abstracts the kernel's internals behind a clean API boundary.

The SDK is not hiding the entity system. Everything it does is
visible in the tree — you can inspect your subscriptions, your
connections, your handlers while your app is running. The SDK
just makes common patterns convenient so you don't reconstruct
execute messages for every tree operation.

The SDK is also a **cohesion mechanism** across language
implementations: not just an API but a shared library of algorithms
that have to produce identical results so that programs constructed
through one SDK can be read and operated on by another. See the
"SDK Layering" section below.

## Design Principles

1. **The API surface is the design.** Whatever we define here is
   what we build. It should be stable enough that the Godot
   binding can adopt the same API shape.

2. **Cross-language in spirit.** The Go, Python, and JS SDKs
   will have similar APIs with language-specific idioms. Rust
   gets ownership semantics, async/await, and trait-based
   extension. Go gets channels and interfaces. But the concepts
   map across.

3. **Smart routing, transparent operation.** The SDK figures out
   how to reach a peer — direct connection, relay chain, local
   shortcut. The developer says what they want, not how to get
   there. But they can inspect the how in the tree if they want.

4. **Correctness first, optimize later.** The initial SDK routes
   everything through the protocol (execute). Once the API is
   stable, hot paths (local tree reads, local subscriptions)
   can be optimized internally without changing the surface.

5. **The kernel is always available.** Developers who need
   entity-core-rust directly can use it. The SDK is a layer on
   top, not a replacement. Some use cases (custom engines, store
   backends, transport implementations) need the kernel.

6. **One SDK, many peers.** The SDK is the environment. Peers
   are resources within it. You don't instantiate multiple
   environments — you work within one and create peers through it.

## SDK Layering

The SDK is not a single layer over the protocol. It is a stack of
three layers that serve different purposes and have different
conformance requirements.

```
┌─────────────────────────────────────────────────────────┐
│  Layer 2 — Algorithm Library / Shared Patterns         │
│  Pipeline builder, bridge framework, content storage,  │
│  type validation, capability attenuation, sync state   │
│  MUST produce identical results across language SDKs   │
├─────────────────────────────────────────────────────────┤
│  Layer 1 — Protocol Facade (dispatched, async)         │
│  get/put/list/remove/has, execute, subscribe, watch,   │
│  discover. Routes through `peer.execute(...)` with     │
│  capability checks. Language-idiomatic; variation OK.  │
│                                                         │
│  Below the facade: L0 direct-store (`store().get/put`) │
│  as an explicit, sync escape hatch per OPERATIONS §2.7 │
├─────────────────────────────────────────────────────────┤
│  Layer 0 — Algorithm Library Foundation                │
│  CBOR canonical encoding, hashing, content-defined     │
│  chunking — shared between core protocol AND SDK       │
└─────────────────────────────────────────────────────────┘
```

### Layer 0 — Foundation Algorithms

Pure functions that the core protocol AND the SDK both depend on.
Must produce identical results across implementations because their
outputs feed content-addressed identity.

Examples:
- CBOR canonical encoding (deterministic for hash stability)
- Content hash function
- Content-defined chunking (variable-size, rolling-hash boundaries
  with exact parameters)
- Path canonicalization
- Base58/Base64 encoding for identifier display

These are not user-facing. They're vendored implementation details
that ensure two peers in different languages produce the same hash
for the same input. The SDK re-exports them as needed.

### Layer 1 — Protocol Facade

Per-peer, per-language ergonomic wrappers over protocol operations.
This is what most of the rest of this document describes —
PeerContext's tree operations, execute, subscribe, discover.

Language variation is expected here:
- Rust: builders, async/await, Result types
- Go: functional options, channels, error returns
- Python: context managers, exceptions, asyncio
- JavaScript: promises, callbacks, event emitters

What must NOT vary: the underlying protocol semantics. Two SDKs
calling `put(path, entity)` must produce the same wire message
and the same emit. Naming differs (`put` / `Put` / `tree_put`);
behavior does not.

### Layer 2 — Algorithm Library / Shared Patterns

Shared algorithms and high-level patterns built on Layer 1.
**Implementations across languages must produce identical results.**

This is the layer most SDK designs miss. It is what makes the
protocol portable in practice — without it, every handler that wants
to (e.g.) chunk content reimplements chunking, and any divergence
defeats deduplication. The SDK has to own these patterns.

Examples:
- **Content storage** — `sdk.content.read(entity)` and
  `sdk.content.write(bytes)` that transparently handle inline vs
  chunked storage. Every implementation chunks identical content
  identically.
- **Pipeline construction** — fluent builder for continuation chains
  that compiles to the same continuation entities regardless of
  which language SDK invokes it.
- **Bridge framework** — generates the standard pair of pipelines
  (KB→external, external→KB) plus sync state tracking for any
  external source bridge (file system, git, S3, HTTP).
- **Capability attenuation** — derives child capability tokens from
  parent grants following the same monotonic restriction rules
  everywhere.
- **Type validation** — validates entities against type definitions
  with the same field/constraint/inheritance semantics.
- **Sync state tracking** — patterns for "last synced at this hash"
  that work the same across implementations.

**Conformance requirement**: Two SDKs in different languages, given
the same Layer 2 call (e.g., `pipeline.build(...)`), must produce
the same entities in the tree. Test vectors validate this.

### What This Means for This Project

This document currently specifies Layer 1 in detail. Layer 0 lives
in the underlying entity-core-rust crates (not the SDK's concern
directly). Layer 2 is mostly future work — but it is **not**
abstract future work. This application specifically needs Layer 2
patterns to wire up relays and subscriptions on the native host
peer without writing continuation entities by hand. See the
"Five Abstraction Levels" section below for where each piece sits.

The SDK is both an API and a **cohesion mechanism** — not just
ergonomic convenience, but the layer that keeps implementations
consistent so that programs (including UI subtrees) constructed
through one SDK can be read and operated on by another.

## Five Abstraction Levels

The SDK surface provides progressive abstraction. Each level builds
on the previous and exists for a specific developer use case.
These are orthogonal to the layering above — Layer 1 spans Levels
0-1 of the abstraction stack, Layer 2 spans Levels 2-5.

### Level 0 — Raw Protocol

Direct EXECUTE/EXECUTE_RESPONSE construction, capability tokens,
wire format. The kernel. Applications normally don't operate here,
but the SDK exposes it via escape hatches (`peer.kernel_peer()`,
`peer.kernel_shared()`).

**Status in this project**: Available, used rarely, for advanced
debugging only.

### Level 1 — SDK Facade (Current State)

Simple wrappers around the protocol. PeerContext exposes two distinct
surfaces per SDK-OPERATIONS.md §2.7:

- **L1 (dispatched, async)**: `ctx.get/put/list/remove/has(...).await`
  — routes through `peer.execute("system/tree", ...)`, capability-checked.
- **L0 (direct store, sync)**: `ctx.store().get/put/...` — bypasses
  dispatch. Every `store()` call is a visible opt-out from the security
  boundary and is used for sync render paths or bootstrapping.

Also at Level 1: `execute`, `subscribe`, `watch`, `discover_handlers`,
`scope(prefix)`. Most of this document describes Level 1 operations on
PeerContext.

**Status in this project**: Production-ready. All windows use L0 via
`store()` today because `handle_action` is sync; the migration to L1
for writes is the next app-level step.

### Level 2 — Typed Entity Builders

Compile-time type checking. Deserialize entities to domain types.
Construct entities from domain types without manual CBOR encoding.
Rust-side: derive macros (`#[derive(Entity)]`); Go-side: generics.

```rust
#[derive(Entity)]
#[entity(type = "knowledge/article")]
struct Article {
    title: String,
    content: String,
}

let article: Article = peer.get("knowledge/articles/foo")?;
peer.put("knowledge/articles/bar", &Article { ... })?;
```

**Status in this project**: Not implemented. Would significantly
reduce boilerplate for windows that have well-defined state structs.
The wiki PoC could motivate this as the first concrete consumer.

### Level 3 — Fluent Pipeline Builder

Continuation chain construction via method chaining. Compiles to
entity subgraphs in the tree. This is "Unix pipes for entities."

```rust
Pipeline::new("relay-setup")
    .then("system/network", "listen")
    .params(json!({"addr": "0.0.0.0:4041"}))
    .then("system/relay", "register")
    .inject("listen_addr")
    .install(&ctx)?;
```

**Status in this project**: Not implemented. **Higher priority for
this app than for the abstract entity system** because the native
host peer needs continuation chains to wire up relays, subscriptions,
and the cross-peer coordination that makes the WASM peer useful.
Without a fluent pipeline builder, every relay/subscription wiring
is manual continuation-entity construction. This is the next major
SDK layer for this project specifically.

### Level 4a — Reactive Bindings via Subscribe + Notify (Working Today)

Subscribe to paths, react to changes. **This is already a complete
reactive framework — no compute extension required.** Watch a path,
get a callback when it changes, re-render or trigger a pipeline.

This is what this project is already doing through the event bridge
and DOM snapshot rebuild loop. The current code wires
`subscribe_events()` → wake function → DOM snapshot rebuild. This is
Level 4a in practice, even if it isn't named that yet.

The next ergonomic improvement is path-pattern subscription with
typed callbacks:

```rust
peer.on_change("knowledge/articles/*", |event| {
    // re-render the article list
})?;
```

**Status in this project**: **Working today.** The event bridge
and generation counter implement this for the full tree. Per-path
subscription helpers are the next step — see SDK migration phases
below.

### Level 4b — Reactive Bindings via Compute Expressions (Future)

Declarative derived values with automatic dependency tracking and
convergence guarantees. Adds spreadsheet semantics on top of 4a.
Requires the compute extension, which is designed but not yet
shipped in entity-core-rust.

```rust
sdk.derive("dashboard/connected-count")
    .depends_on("system/peer/*/status")
    .compute(|peers| count_where(peers, |p| p.connected))
    .install(&ctx)?;
```

**Status in this project**: Future. Becomes available when compute
extension lands in the kernel. Not on the near-term path.

### Level 5 — Entity-Native Programs

UI, business logic, and coordination expressed as entity subtrees.
The program IS data in the tree. Portable across implementations
because any peer with the right type renderers can execute it.

```
ui/knowledge-base/
  layout: split horizontal 0.3
  children:
    left:
      type: view/list
      source: subscribe("knowledge/articles/*")
      on_select: write("ui/knowledge-base/selected", $path)
    right:
      type: view/markdown
      source: lookup(read("ui/knowledge-base/selected"))
```

**Status in this project**: Long-term goal. Each renderer (this
project, Godot Studio, the Go workbench canvas) implements type
renderers for the standard view types; the UI subtree is portable.
Tracks toward "build once, runs in any renderer." This is the
endpoint of the cross-team UI substrate work.

## Core Types

Two types define the SDK's interface:

```rust
/// The environment for entity-native application development.
///
/// Manages a collection of peers, shared configuration, and
/// transport connectors. One instance per application.
/// Peers are created and managed through the SDK.
pub struct EntitySDK {
    // internal: HashMap<String, PeerContext>, connector, config
    // none of this is public
}

/// A per-peer handle for entity operations.
///
/// Wraps a kernel Peer with ergonomic methods for tree ops,
/// subscriptions, connections, and handler management.
/// Obtained from EntitySDK via sdk.peer(peer_id).
/// All operations go through the entity protocol — PeerContext
/// never exposes internal storage or indexes directly.
pub struct PeerContext {
    // internal: Peer, PeerShared, peer_id, generation, wake_fn
    // none of this is public
}
```

`EntitySDK` is what you initialize once. `PeerContext` is what
you work with day-to-day. Simple applications create one peer
and use its context exclusively. Multi-peer applications
(like the Entity Browser) create several and manage them
through the SDK.

## Lifecycle

```rust
// -- SDK construction --

let sdk = EntitySDK::builder()
    .connector(connector)             // transport for outbound connections
    .config(SdkConfig { ... })        // shared configuration
    .build()?;

// -- Create peers through the SDK --

let app_peer_id = sdk.create_peer(app_keypair, PeerMetadata::primary("app"))?;
let user_peer_id = sdk.create_peer(user_keypair, PeerMetadata::primary("user"))?;
// For a random keypair, use the SDK builder's `generate_keypair()` when
// constructing the SDK, or generate a Keypair externally and pass it in.

// -- Get per-peer handles --

let app = sdk.peer(&app_peer_id).unwrap();
let user = sdk.peer(&user_peer_id).unwrap();

// -- Peer lifecycle --

let all: Vec<&str> = sdk.peer_ids();
sdk.remove_peer(&ephemeral_id)?;

// -- Shutdown --
// SDK cleans up on drop: stops engines, closes connections.
// Explicit shutdown available if needed:
sdk.shutdown().await;
```

### Simple case (single peer)

For applications that only need one peer, the builder can
create it directly:

```rust
let sdk = EntitySDK::builder()
    .generate_keypair()               // creates a default peer
    .build()?;

// Default peer context available immediately:
let peer = sdk.default_peer();           // infallible — builder guarantees a default
peer.get("some/path").await?;            // L1 dispatched (async)
peer.store().get("some/path");           // L0 direct (sync, escape hatch)
```

The default peer is the one created by the builder's keypair.
This keeps the single-peer case simple while supporting
multi-peer without architecture changes.

## PeerContext Operations

### Tree Operations

The core data operations. PeerContext operates on its own
peer's tree. For remote paths, use `entity://` URIs through
execute.

Two surfaces, per SDK-OPERATIONS.md §2.7:

```rust
// -- L1: dispatched, async, capability-checked --

let entity: Option<Entity> = peer.get("path/to/entity").await?;
let hash: Hash            = peer.put("path/to/entity", entity).await?;
let entries: Vec<ListingEntry> = peer.list("path/prefix/").await?;
let removed: bool         = peer.remove("path/to/entity").await?;
let exists: bool          = peer.has("path/to/entity").await?;

// -- L0: direct store, sync, bypasses dispatch (escape hatch) --

let store = peer.store();
let entity: Option<Entity> = store.get("path/to/entity");
let hash: Hash             = store.put("path/to/entity", entity)?;
let entries                = store.list("path/prefix/");
let removed: bool          = store.remove("path/to/entity");
let exists: bool           = store.has("path/to/entity");
```

Every `store()` call is an explicit opt-out from the security
boundary. Prefer L1 unless you are in a sync render loop or
bootstrapping state before the runtime is running.

#### ListingEntry

```rust
/// An entry in a tree listing.
pub struct ListingEntry {
    pub path: String,
    pub hash: Hash,
    pub entity_type: Option<String>,
}
```

#### Scoped handles

For prefix-bound ergonomics, use `scope(prefix)`:

```rust
let ws = peer.scope("app/entity-browser/workspace");
ws.put("windows/1/state", entity)?;   // writes at the absolute-qualified path
let sub = ws.scope("windows/1");      // sub-scope
```

### Subscriptions

Subscribe to tree changes on this peer. The PeerContext handles
the wiring — whether that's a local broadcast receiver, a
subscription engine chain, or a relay-routed remote subscription.
The developer just gets callbacks.

> **This is Level 4a — the reactive bindings layer — and it's
> already working today.** The current implementation wires
> `peer.subscribe_events()` (broadcast receiver) → wake function →
> DOM snapshot rebuild. That IS a complete reactive framework, just
> at the whole-tree granularity. The next step is path-pattern
> subscription with typed callbacks (the API below). The compute
> extension (Level 4b) adds declarative derived values on top of
> this layer but is not required to ship reactive bindings today.

```rust
// -- Callback style (simplest) --

let handle = peer.on_change("system/type/**", |event: ChangeEvent| {
    println!("changed: {} ({:?})", event.path, event.change_type);
})?;

// Cancel when done:
handle.cancel();

// -- Async stream style --

let mut stream = peer.subscribe("workspace/ui/**").await?;
while let Some(event) = stream.next().await {
    // process event
}

// -- Multiple patterns --

let handle = peer.on_change("system/handler/**", on_handler_change)?;
let handle2 = peer.on_change("workspace/settings/**", on_settings_change)?;

// -- Remote subscriptions (same API, through execute) --

let handle = peer.on_change(
    "entity://{remote_pid}/system/type/**",
    on_remote_type_change,
)?;
// PeerContext internally: checks direct connection, checks relay network,
// wires subscription chain through best available path.
// Developer doesn't see this. If network topology changes, PeerContext
// can re-route (future: continuation-based resilience).
```

Subscriptions are per-peer — you subscribe to changes on specific
peers and only get notified for those. No blanket "any peer changed"
aggregation. If you want to monitor every peer, you explicitly
subscribe to each one. The default is targeted notification.

#### ChangeEvent

```rust
/// A tree change notification.
pub struct ChangeEvent {
    pub path: String,
    pub hash: Hash,
    pub previous_hash: Option<Hash>,
    pub change_type: ChangeType,        // Created, Modified, Deleted
}
```

### Render integration

For UI applications that need a "something changed, redraw"
signal rather than per-event callbacks:

```rust
// Register a wake function — called when ANY subscribed path changes.
// Coalesces: 1000 changes between frames = 1 wake call.
// Set per-peer — each peer has its own wake signal.
peer.set_wake_fn(|| {
    // entity-browser-rust (WASM/DOM): flip the dirty flag so the next
    // requestAnimationFrame frame rebuilds. (Per window, WindowWatch wraps
    // ctx.store().subscribe(prefix, cb) to wire this automatically.)
    request_repaint();
});

// Or for Godot:
peer.set_wake_fn(|| {
    // emit signal or set dirty flag
});
```

The wake function is the render-framework bridge. It's one
function per peer, set once. The event bridge (already
implemented in sdk.rs) connects tree change events to the
wake function.

### Connections

```rust
// -- Connect to a remote peer --

let remote_pid: String = peer.connect("ws://192.168.1.5:4041").await?;

// -- Listen for incoming connections (native only) --

peer.listen("0.0.0.0:4041").await?;
// PeerContext starts accepting connections in background.
// New peers appear in the tree and are accessible via entity:// URIs.

// -- List connected peers --

let peers: Vec<PeerInfo> = peer.connected_peers();

// -- Connection events --

let handle = peer.on_peer_event(|event: PeerEvent| {
    match event {
        PeerEvent::Connected { peer_id, addr } => { ... }
        PeerEvent::Disconnected { peer_id, reason } => { ... }
    }
});
```

#### PeerInfo

```rust
pub struct PeerInfo {
    pub peer_id: String,
    pub addr: Option<String>,
    pub direction: Direction,           // Inbound, Outbound
}
```

### Execute (Power Tool)

Direct handler execution for operations not covered by
convenience methods. This is the "drop down a level" API —
you're constructing protocol operations directly.

```rust
let result: HandlerResult = peer.execute(
    "system/tree",                      // handler URI
    "get",                              // operation
    params,                             // Entity (parameters)
).await?;

// With options (resource targeting, capability tokens):
let result = peer.execute_with_options(
    "custom/handler",
    "process",
    params,
    ExecuteOptions {
        resource: Some(ResourceTarget { ... }),
        ..Default::default()
    },
).await?;
```

### Handler Discovery

Read what handlers are available on a peer — local or remote.

```rust
// -- Local handlers --

let handlers: Vec<HandlerInfo> = peer.discover_handlers().await?;
for h in &handlers {
    println!("{}: {} (ops: {:?})", h.pattern, h.name, h.operations);
}

// -- Remote handlers --

let handlers = peer.discover_handlers_on(remote_pid).await?;
```

#### HandlerInfo

```rust
pub struct HandlerInfo {
    pub pattern: String,                // e.g., "system/tree"
    pub name: String,                   // human-readable
    pub operations: Vec<String>,        // e.g., ["get", "put", "snapshot"]
}
```

### Handler Registration

Register application handlers on this peer at runtime. This is how
your application becomes a participant in the entity protocol — other
peers can execute operations on your handlers.

Implements SDK-OPERATIONS §11.5: the SDK primitive couples the
protocol-side declaration (interface + handler + optional grant tree
entities) with the implementation-side binding (callable in the
dispatch index). Tree writes happen first; compensation on failure;
close (explicit or via `Drop`) reverses both halves.

```rust
use crate::register_handler::{HandlerSpec, OperationSpec, HandlerBody};
use std::sync::Arc;

// -- Build a spec and a closure body --

let spec = HandlerSpec::new(
    "app/myapp/processor",              // bare pattern, no leading slash
    "processor",
    vec![OperationSpec::new("process")],
);

let body: HandlerBody = Arc::new(|ctx| {
    Box::pin(async move {
        match ctx.operation.as_str() {
            "process" => {
                // ... do work, return HandlerResult::ok(result_entity)
                # unimplemented!()
            }
            op => Err(HandlerError::NotSupported(op.into())),
        }
    })
});

// -- Register: returns a RegisteredHandler that owns the lifecycle --

let registered = peer.register_handler(spec, body)?;

// -- Unregister: drop the handle, or call close() explicitly --

registered.close();       // idempotent; Drop also calls this
// or simply: drop(registered);
```

The bare pattern is qualified internally to `/{peer_id}/{pattern}`. A
leading slash is an error. Supply `spec.with_internal_scope(...)` to
declare the paths/handlers the body needs to reach — the SDK mints a
self-grant at `/{peer_id}/system/capability/grants/{pattern}`.

**Collisions** return 409 `pattern_collision`. **Invalid specs** (empty
pattern, empty operations) return 400 `invalid_handler_spec`. On
partial-write failure the SDK compensates (reverses earlier writes)
and returns 500 `partial_registration_failure`.

### Workspace Paths — Application Layer

**The SDK does not define application namespace conventions.** It
provides generic primitives only: `get`/`put`/`list`/`remove`/`has`
(L1 dispatched, async) and `store()` (L0 direct, sync). Applications
define their own path helpers.

For `entity-browser`, these live in `src/app_paths.rs`:

```rust
use crate::app_paths;

// Per-window state path:
app_paths::window_state_path(peer_id, window_id)
// → "/{peer_id}/app/entity-browser/workspace/windows/{id}/state"

// Global shared state path:
app_paths::settings_path(peer_id, "ui")
// → "/{peer_id}/app/entity-browser/settings/ui"
```

The namespace (`app/{app-id}/workspace/…`, `app/{app-id}/settings/…`)
follows the domain convention in
`GUIDE-PEER-CONCERNS-AND-NAMESPACES.md`. Other apps built on the same
SDK define their own `APP_ID` and their own helpers.

For prefix-bound ergonomics inside a handler or view, the SDK exposes
`PeerContext::scope(prefix)`:

```rust
let ws = ctx.scope("app/entity-browser/workspace");
ws.put("windows/1/state", entity)?;  // writes absolute-qualified path
```

### Future: Handler utilities

These are patterns that show up repeatedly in handler
development. Not in the first SDK cut, but on the horizon:

```rust
// Generic handler from a closure (no trait impl needed):
peer.handle("app/echo", &["echo"], |ctx| async {
    Ok(HandlerResult::ok(ctx.params.clone()))
})?;

// Type registration (register entity type definitions):
peer.register_type(TypeDefinition {
    name: "doc/paper",
    extends: Some("doc/base"),
    fields: vec![ ... ],
})?;
```

## EntitySDK Methods

The SDK is a multi-peer container. Current surface:

```rust
impl EntitySDK {
    // Peer lifecycle
    fn create_peer(
        &mut self,
        keypair: Keypair,
        metadata: PeerMetadata,
    ) -> Result<&str, SdkError>;
    fn register_backend_peer(&mut self, peer_id: String, metadata: PeerMetadata) -> bool;
    fn remove_peer(&mut self, peer_id: &str) -> bool;

    // Lookup
    fn peer(&self, peer_id: &str) -> Option<&PeerContext>;
    fn peer_ids(&self) -> Vec<&str>;
    fn has_peer_context(&self, peer_id: &str) -> bool;
    fn default_peer(&self) -> &PeerContext;          // infallible
    fn default_peer_id(&self) -> &str;

    // Metadata
    fn peer_metadata(&self, peer_id: &str) -> Option<&PeerMetadata>;
    fn set_metadata(&mut self, peer_id: &str, meta: PeerMetadata);

    // Diagnostics
    fn generation(&self) -> u64;
}
```

For cross-peer operations, look the PeerContext up via `sdk.peer(id)`
and call its async `get/put/list` methods (L1) or `store()` for L0.
There is no `tree_*_on` convenience — `sdk.peer(id)?.get(path).await`
is the idiomatic form.

## Type System Access (Future Tier)

Reading and working with entity type definitions. This is how
applications understand what entities mean, not just what data
they contain.

```rust
// -- Read a type definition --

let type_def: Option<TypeDef> = peer.type_get("doc/paper").await?;
if let Some(t) = type_def {
    println!("extends: {:?}", t.extends);
    for field in &t.fields {
        println!("  {}: {}", field.name, field.field_type);
    }
}

// -- List known types --

let types: Vec<String> = peer.type_list().await?;

// -- Check type hierarchy --

let is_doc: bool = peer.type_extends("doc/paper", "doc/base").await?;
```

## Continuations and Pipelines (Near-Term Need)

Chaining operations, relay setup, subscription wiring, error
recovery, resume-on-disconnect. This is Level 3 in the abstraction
stack and **Layer 2 territory** — shared algorithm library, not
just per-language ergonomics.

**Why this is more important for this project than for the entity
system in the abstract**: this application depends on a native host
peer (the Tauri backend, or a standalone service) to set
up relays and subscriptions on its behalf. The WASM peer can't
listen, can't reach TCP-only peers, can't run background daemons —
the native host has to orchestrate all of that. Without a fluent
pipeline builder, every relay registration, every subscription
chain, every cross-peer wiring step has to be done by manually
constructing continuation entities. That doesn't scale and it
doesn't survive the SDK boundary.

The pipeline builder is the right abstraction. It compiles
high-level intent into the same continuation entities you'd
construct by hand, but with type checking, named steps, and
capability scoping.

### Sketch of the API direction

```rust
// Set up the WASM peer to relay through the backend native peer:
let pipeline = Pipeline::new("backend-relay")
    .then("system/network", "listen")
    .params(json!({"addr": "0.0.0.0:4041"}))
    .then("system/relay", "register")
    .inject("listen_addr")
    .then("system/capability", "grant")
    .params(json!({"to": wasm_peer_id, "scope": "relay/*"}))
    .install(&ctx)?;

// Subscribe to a remote path through the relay, with automatic
// resume-on-disconnect:
let handle = peer.subscribe_resilient(
    "entity://{remote}/knowledge/articles/*",
    SubscribeOptions {
        retry_on_disconnect: true,
        fallback_via_relay: true,
    },
).await?;

// Simple chain: put entity, then subscribe to it
let hash = peer.put("knowledge/articles/foo", entity).await?;
peer.on_change("knowledge/articles/foo", handle_update)?;
```

### Why it's Layer 2 (not just Layer 1)

A pipeline builder is not just a Rust convenience. The continuation
entities it produces must match across implementations — a Go peer,
a Rust peer, and a Python peer all building "set up backend relay"
pipelines must produce the same continuation chain entities, the
same capability scoping, the same callback path structure.
Otherwise sync breaks: two peers with the "same" relay setup
diverge in the tree, and content-addressing stops dedup.

The fluent builder API can vary by language (Rust traits, Go
chained methods, Python context managers). The output continuation
entities must not.

### What's currently manual

Right now, the Tauri backend peer architecture is described in
`reviews/DEPLOYMENT-ARCHITECTURE.md` as a sequence of manual steps:
backend creates peer → backend starts WS listener → WASM auto-connects
→ backend grants capabilities → ... Each step is a separate hand-rolled
EXECUTE call from the Tauri shell. With Level 3 in place, the same
sequence becomes one pipeline declaration.

This is the next major SDK direction for this project, and it
unblocks the relay + subscription work that the deployment review
identifies as the biggest "connective fabric" gap.

## What the SDK Does NOT Expose

These are kernel internals. Application code should not need them:

| Kernel concept | Why hidden | SDK alternative |
|---------------|-----------|-----------------|
| `ContentStore` | Direct storage bypass | `peer.get()`, `peer.put()` (or `peer.store()` for sync) |
| `LocationIndex` | Direct index bypass | `peer.list()`, `peer.has()` |
| `PeerShared` | Internal wiring struct | Not needed |
| `RemoteState` | Connection pool internals | `peer.connected_peers()` |
| `broadcast::Receiver` | Notification plumbing | `peer.on_change()`, `peer.subscribe()` |
| `make_execute_fn` | Internal dispatch builder | `peer.execute()` |
| `NotifyingLocationIndex` | Store decorator | Automatic — PeerContext wires this |
| `SubscriptionEngine` | Notification routing | `peer.on_change()` handles it |

If you need these, use entity-core-rust directly. PeerContext
doesn't prevent it — you can access the underlying peer for
advanced use:

```rust
// Escape hatch: access the kernel peer directly
let kernel_peer: &Peer = peer.kernel_peer();
let shared: Arc<PeerShared> = peer.kernel_shared();

// This is for advanced use: custom engines, store backends,
// transport implementations, debugging.
```

## Cross-Language API Mapping

The same concepts appear in every language SDK. The surface
adapts to language idioms:

| Concept | Rust | Go | Python | JS |
|---------|------|-----|--------|-----|
| SDK init | `EntitySDK::builder().build()` | `entitysdk.New(opts)` | `EntitySDK(**opts)` | `new EntitySDK(opts)` |
| Create peer | `sdk.create_peer(kp)` | `sdk.CreatePeer(kp)` | `sdk.create_peer(kp)` | `sdk.createPeer(kp)` |
| Get peer | `sdk.peer(pid)` | `sdk.Peer(pid)` | `sdk.peer(pid)` | `sdk.peer(pid)` |
| Tree get (L1) | `peer.get(path).await?` | `peer.Get(path)` | `await peer.get(path)` | `await peer.get(path)` |
| Tree get (L0) | `peer.store().get(path)` | `peer.Store().Get(path)` | `peer.store.get(path)` | `peer.store.get(path)` |
| Subscribe | `peer.on_change(pat, \|e\| {...})?` | `peer.OnChange(pat, func)` | `peer.on_change(pat, fn)` | `peer.onChange(pat, fn)` |
| Connect | `peer.connect(addr).await?` | `peer.Connect(addr)` | `await peer.connect(addr)` | `await peer.connect(addr)` |
| Execute | `peer.execute(h, op, p).await?` | `peer.Execute(h, op, p)` | `await peer.execute(h, op, p)` | `await peer.execute(h, op, p)` |
| Wake signal | `peer.set_wake_fn(\|\| {...})` | `peer.SetWakeFunc(fn)` | `peer.set_wake_fn(fn)` | `peer.setWakeFn(fn)` |
| List peers | `sdk.peer_ids()` | `sdk.PeerIDs()` | `sdk.peer_ids()` | `sdk.peerIds()` |

Go's version is synchronous (goroutines handle concurrency
internally). Rust, Python, JS are async. The callback patterns
vary (closures, channels, async generators) but the concepts
are the same.

Go currently has `AppPeer` wrapping a single peer with the
application managing `[]managedPeer`. The Go SDK could adopt the
same model: `entitysdk.SDK` manages peers, `PeerContext` is per-peer.

## Implementation Strategy

### Phase 1: Rename and restructure — COMPLETE

- ✅ Rename current `EntitySDK` → `PeerContext`
- ✅ Create new `EntitySDK` that holds `BTreeMap<String, PeerContext>`
- ✅ Move connector and config into new EntitySDK
- ✅ PeerManager delegates to `self.sdk` (new multi-peer SDK)
- ✅ PeerManager's `_peer_id` parameters become real, route through SDK
- ✅ Window code unchanged — still calls PeerManager with peer_id

### Phase 2: Multi-peer activation — COMPLETE

- ✅ `sdk.create_peer()` / `sdk.remove_peer()` working
- ✅ Spawn a second peer to prove multi-peer works end-to-end
- ✅ Entity Tree window can open bound to any peer
- ✅ Per-peer event bridges and generation counter

### Phase 3: Access-level split (COMPLETE)

The security boundary is now visible in the SDK surface per
`SDK-OPERATIONS.md` §2.7:

- ✅ **L1 (dispatched, async)**: `ctx.get/put/list/remove/has` route
  through `peer.execute("system/tree", ...)`, capability-checked
- ✅ **L0 (direct store, sync)**: `ctx.store()` returns a `StoreAccess`
  handle with sync get/put — every call is a visible escape hatch
- ✅ Application migrated: all L0 usages explicitly go through `store()`
- [ ] **App-side async migration**: `handle_action` is still sync, so
  view writes run at L0 via `store()`. Making the action pipeline async
  unlocks L1 for writes and is the next app-level step.

### Phase 4: Fine-grained subscriptions (Level 4a polish)

The reactive framework already works at whole-tree granularity via
`subscribe_events()` + generation counter. This phase adds
path-pattern subscriptions with typed callbacks so windows only
rebuild when their watched paths change.

- [ ] `peer.on_change(pattern, callback)` API on PeerContext
- [ ] Subscription handle lifecycle (cancel, rate limit)
- [ ] Per-window subscriptions instead of global generation counter
- [ ] Integration with the subscription extension for cross-peer
  notifications

### Phase 5: Pipeline builder (Layer 2 — first major shared algorithm)

This is the next major SDK layer, motivated by this project's
relay/subscription wiring needs. See "Continuations and Pipelines"
above for the rationale.

- [ ] `Pipeline::new(name)` builder type
- [ ] `.then(handler, operation)` step chaining
- [ ] `.params(...)`, `.inject(field)`, `.transform(...)` step config
- [ ] `.install(&ctx)` writes continuation entities to tree and
  dispatches initial EXECUTE
- [ ] `PipelineHandle` for inspection, suspension, cancellation
- [ ] Test vectors validating identical entity output across
  hypothetical Go/Rust implementations
- [ ] Used by the Tauri backend peer setup flow as the first
  real consumer

Single-step ("just an EXECUTE wrapper") can ship first as the MVP.
Multi-step + transforms + on-error chains follow.

### Phase 6: Handler development helpers

- [x] `register_handler` — paired tree + dispatch-index writes (SDK-OPERATIONS §11.5)
- [x] Closure-based handler convenience (`HandlerBody` closure, no trait impl needed)
- [x] `RegisteredHandler::close` / Drop for explicit + scoped unregistration
- [ ] Type registration helpers (the `types` field on `HandlerSpec` is not yet wired)
- [x] Handler discovery on local peer (`discover_handlers()`)
- [ ] Handler discovery on remote peers (`discover_handlers_on(remote_pid)`)

### Phase 7: Typed entity builders (Level 2)

- [ ] `#[derive(Entity)]` macro for compile-time type checking
- [ ] `peer.get::<T>(path)` and `peer.put(path, &value)`
- [ ] Type registration helpers from struct definitions

### Phase 8: Layer 2 expansion

Additional Layer 2 algorithms beyond the pipeline builder. Each
must be specified with reference test vectors so cross-language
implementations agree.

- [ ] `sdk.content.read/write` content abstraction (inline → chunked
  later, transparent to handlers)
- [ ] Bridge framework (`sdk.bridge.create(config)`) for KB/file
  system, KB/git, KB/HTTP
- [ ] Capability attenuation helpers
- [ ] Type validation against type definitions
- [ ] Sync state tracking patterns

### Phase 9: Extract to crate

- [ ] `entity-sdk-rust` crate
- [ ] This project and Godot depend on it
- [ ] The API surface is the crate boundary
- [ ] Conformance test vectors for Layer 2 published with the crate

## Relationship to Current Code

This project (entity-browser-rust) is the first consumer of the
SDK design described above. The SDK rename and multi-peer
restructuring is complete. Current code maps to the SDK concepts as
follows:

| Current code | SDK concept |
|-------------|-------------|
| `EntitySDK` / `PeerManager` (entity-sdk crate, upstream) | Multi-peer container (`BTreeMap<String, PeerContext>`); `PeerManager` is the per-SDK manager wrapped by `Sdk::Direct`/`Sdk::Worker` |
| `PeerContext` (entity-sdk crate) | Per-peer handle — L1 dispatched ops, L0 `store()` access, scope, subscriptions, event bridge, generation counter |
| `Peers` (peers.rs, this repo) | **Application layer** — the multi-SDK router (`sdks: Vec<Sdk>` + `peer_routes`); app-tier state (event log, connections, listener addr) is **tree-backed**, not struct fields |
| `app_paths::window_state_path()` / `settings_path()` (app_paths.rs) | Application-layer path conventions — the SDK is namespace-agnostic |
| `ctx.event_bridge()` | Per-peer wake signal wiring (native + WASM variants) |
| `ctx.store().subscribe(prefix, cb)` (via `WindowWatch`) | Subscription-driven dirty flags (the snapshot/`generation()` polling was removed in Phase 4) |

### What's still in flight

- **Async action pipeline**: `handle_action` is sync, so view writes
  use L0 via `store()`. Making the dispatch pipeline async lets writes
  migrate to L1 `ctx.put().await`.
- **Per-path subscriptions** (Phase 4): the generation counter +
  whole-tree event bridge works but is coarse. Per-window
  path-pattern subscriptions, routed through the subscription
  extension for cross-peer filtering, are the polish.
- **Pipeline builder** (Phase 5): not yet started. This is what
  unblocks the relay/subscription wiring on the native host peer.
- **Grant lifecycle**: `GrantScope` / `GrantInfo` data types exist;
  the `create_grant`/`delegate_grant`/`revoke_grant`/`inspect_grants`
  operations need wiring through the capability handler.

The window layer and DOM layer are insulated from these changes
because the PeerManager API shape stays stable — only its
implementation evolves.

## What PeerContext Provides vs Kernel Peer

PeerContext is NOT just a renamed Peer. It adds:

| Concern | Kernel Peer | PeerContext |
|---|---|---|
| Tree ops | Direct store/index access | L1 dispatched (async, via `execute`) + L0 direct (sync, via `store()`) — SDK-OPERATIONS §2.7 |
| Scoped handles | None | `scope(prefix)` for prefix-bound ops |
| Watch | None | `watch(pattern)` pull-based change stream (native) |
| Subscriptions | Raw broadcast receiver | `subscribe(prefix, callback)` + `subscribe_events()` |
| Handler discovery | Scan index yourself | `discover_handlers()` convenience |
| Shared state | `peer.shared()` (creates new!) | Cached, created once |
| Identity | `peer.peer_id()` (PeerId type) | `peer_id()` as `&str` |
| Generation tracking | None | Per-peer generation counter for snapshot detection |
| Wake signal | None | `set_wake_fn()` + `event_bridge()` |
| Diagnostics | Manual | `entity_count()`, `path_count()` |

## References

- SDK-Peer Boundary analysis (historical): `docs/architecture/reviews/legacy/SDK-PEER-BOUNDARY.md`
- Multi-Peer Analysis: `docs/archive/reviews/MULTI-PEER-ANALYSIS.md`
- Peer Identity Model: `docs/architecture/reviews/PEER-IDENTITY-MODEL.md`
- Entity App Framework: `docs/architecture/specs/ENTITY-APP-FRAMEWORK.md`
- Window Architecture: `docs/architecture/specs/WINDOW-ARCHITECTURE.md`
- Implementation Roadmap: `docs/plans/IMPLEMENTATION-ROADMAP.md`
- Entity Core Rust (kernel): `../entity-core-rust/`
- Go SDK direction: `../entity-workbench-go/workbench/` (Executor, PeerContext, DataContext)
- Go framework docs: `../entity-workbench-go/docs/architecture/ENTITY-APPLICATION-FRAMEWORK.md`
- Entity Protocol: `../entity-core-architecture/docs/architecture/v7.0-core-revision/specs/ENTITY-CORE-PROTOCOL-V7.md`
- SDK exploration (academic team): `../entity-core-papers/papers/shared/notes/architecture-implementation/exploration-sdk-and-ergonomics.md`

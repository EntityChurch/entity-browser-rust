# Window Architecture — Multi-Window + Entity-Backed State

> **Note.** This spec describes the window-system model. The
> authoritative "add a window" pattern lives in the code and
> `../guides/DEVELOPER-GUIDE.md`. The lower sections (factory tables,
> file map) are illustrative and may drift from the exact source
> layout.

**Status**: Working. Nine window types, multi-instance, command palette.
Entity-backed state for all windows. **DOM-only.** All windows are on
the model/output/render split; reactivity is subscription-driven via
`WindowWatch`.

Tree namespace uses `app/entity-browser/` per the SDK domain spec.
Path conventions live in `src/app_paths.rs` (application layer).

## Core Architecture

### WindowView Trait

Each window type implements a common interface:

```rust
trait WindowView {
    fn title(&self) -> String;
    fn type_name(&self) -> &'static str;
    fn peer_id(&self) -> &str { "" }    // for cleanup routing

    fn handle_action(&mut self, action: &Action, peers: &Peers);

    fn render_dom(&self, container: &web_sys::Element,
                  state: &Peers, ctx: &DomCtx);
}
```

**DOM-only.** There is no native pixel-canvas render path; the
earlier `show_canvas` / egui path and the native eframe renderer have
been removed. WASM builds are the only build (DOM in a browser, DOM
in a Tauri WebView). The current WASM binary is ~4.8MB.

`handle_action` receives `&Peers` so windows can read/write
entity-backed state from the tree (Direct arm via
`peer_context_or_default(peer_id)`; cross-arm ops via router methods).

### Window Struct Pattern

Every window struct holds only identity — no application state:

```rust
pub struct MyWindow {
    window_id: WindowId,
    peer_id: String,
}
```

State is read from and written to the tree via `read_state()` /
`write_state()` helpers defined on each window.

### WindowManager

```rust
struct WindowManager {
    windows: Vec<WindowInstance>,
    next_id: WindowId,
    types: Vec<WindowType>,
}
```

Factory: `WindowType::create` receives `(WindowId, &PeerManager)` and
returns `Box<dyn WindowView>`. The factory writes initial state to the
tree at spawn time.

### Command Palette

Lists available window types with spawn buttons, and active instances
with close buttons. Rendered as a collapsible `<details>` in DOM.
Active entries are clickable to scroll to that window section.

## Entity-Backed State

### Tree structure

```
{peer_id}/app/entity-browser/
  settings/
    ui                       <- global (shared across all Settings windows)
  workspace/
    windows/
      1/state                <- per-window (unique to instance 1)
      1/results              <- per-window results (reserved)
      2/state                <- per-window (unique to instance 2)
      3/state                <- per-window (unique to instance 3)
```

Path conventions live in `src/app_paths.rs` (application layer), not in
the SDK. The SDK is generic — it has no knowledge of `entity-browser`,
window IDs, or workspace layout. Other applications using the same SDK
define their own namespace.

### State lifecycle (subscription-driven)

1. **Spawn** — factory writes initial state entity to the tree
   *and* installs a `WindowWatch` (`src/window_watch.rs`) wrapping
   the SDK's L0 `ctx.store().subscribe(prefix, callback)` primitive.
   The watch subscribes to every tree path the window's render
   reads (per-window state, any global state it consumes).
2. **Render** — `render_dom` reads its model from the tree
   (L0 `store().get`) and builds DOM. The DOM renderer checks the
   window's dirty flag; clean windows are skipped entirely.
3. **Interact** — DOM event handler pushes an `Action` →
   `handle_action` calls `read_state`/modifies/`write_state` (`put`
   under the hood — L0 today; L1 once the action pipeline is async).
4. **Refresh** — every `put` (local or remote, via the event bridge)
   fires the matching subscription callbacks, which flip the
   relevant window's dirty flag. Next frame the DOM renderer
   rebuilds **only the dirty section** — there is no global
   generation counter and no full-tree snapshot.
5. **Close** — `process_actions` in `app.rs` removes the per-window
   tree paths and drops the watch (subscriptions cancel automatically).

The cross-window state-extraction pattern (model / output /
renderer) comes in three model shapes (cached, mirrored,
pass-through). Subscription rewiring drives reactivity; window
registry signalling goes through `peer_registry::sync()`.

### Global vs per-window

**Per-window** (Entity Tree, Execute Console, Peer Connections, Query Console):
- Path: `app_paths::window_state_path(peer_id, self.window_id)`
  → `/{peer_id}/app/entity-browser/workspace/windows/{id}/state`
- Each instance has independent state
- Cleaned up from tree on close

**Global** (Settings):
- Path: `app_paths::settings_path(peer_id, "ui")`
  → `/{peer_id}/app/entity-browser/settings/ui`
- All instances read/write the same entity
- Uses `ensure_state` pattern — write defaults only if absent
- Persists across window instances (second Settings window does not
  overwrite first window's changes)

### State serialization

Each window defines a state struct with CBOR conversion:

```rust
struct MyState { field: String }

impl MyState {
    fn from_entity(entity: &Entity) -> Self { /* decode CBOR map */ }
    fn to_entity(&self) -> Entity { /* encode CBOR map */ }
}
```

Entity types use the `app/state/` prefix per the SDK domain
namespace conventions, e.g., `app/state/setting`,
`app/state/entity_tree`, `app/state/execute_console`.

### SDK integration

Entity-backed state flows through the SDK. The SDK exposes two access
levels (see `ENTITY-SDK-API.md` for the L0/L1 operations split):

- **L1 (dispatched, async)**: `ctx.get(path).await`, `ctx.put(path, e).await`
  — routed through `peer.execute("system/tree", ...)`, capability-checked.
- **L0 (direct store, sync)**: `ctx.store().get(path)`, `ctx.store().put(path, e)`
  — bypasses dispatch. Every `store()` call is a visible opt-out from the
  security boundary.

Because `handle_action` is currently synchronous, views use L0 via
`store()` in both reads and writes. Making the action pipeline async
is the remaining gap — once done, writes migrate to `ctx.put().await`.

```
Window              PeerContext (L0 store)      Peer Tree
  |                     |                         |
  |-- read_state() ---->|                         |
  |                     |-- store().get(path) --->|
  |<--- entity ---------|<--- entity -------------|
  |                     |                         |
  |-- write_state() --->|                         |
  |                     |-- store().put(path,e) ->|
  |                     |-- broadcast event ----->|
  |                     |                         |
  |   (subscription callback flips window dirty flag)
  |<--- per-window DOM rebuild ---|                |
```

Every `put` (L0 or L1) broadcasts a tree-mutation event. Windows
that subscribed to the mutated prefix have their dirty flag flipped;
the next frame the DOM renderer rebuilds only those windows. The
generation counter / global snapshot mechanism has been removed.

On WASM, the event bridge (`ctx.event_bridge()`) delivers remote
mutations into the same broadcast path, so remote changes drive
dirty flags identically.

Windows access their `PeerContext` via
`peers.peer_context_or_default(&self.peer_id)`. Path construction
goes through `app_paths::*` helpers, not the SDK — the SDK provides
generic primitives, the application defines namespace conventions.

## DOM Rendering Model

> **Historical.** The snapshot/generation full-rebuild model
> described below has been removed; reactivity is subscription-driven
> (`WindowWatch` → per-window dirty flag → per-window rebuild). The
> text below describes how it used to work and is kept for context
> only.

### Snapshot-based full rebuild (historical, removed)

`DomRenderer::render()` builds a snapshot string each frame:

```
"{window_id}:{type_name};...gen:{generation},log:{log_len},conn:{conn_count}"
```

If the snapshot matches the previous frame → skip rebuild entirely.
If different → clear all DOM, drop old closures, rebuild everything.

This is intentionally simple. At current scale (9 window types, low
hundreds of entities) the full rebuild is fast. The snapshot includes
the SDK generation counter, which captures all tree mutations (local
`put` and remote changes via event bridge).

### Shadow DOM encapsulation

All DOM rendering happens inside a Shadow DOM attached to `#dom-layer`.
CSS styles are injected into the shadow root for **style isolation** — window
styles and the host page (and any embedded-app iframe chrome) can't leak into
each other, and theme custom properties inherit cleanly through the one root.

### Closure lifecycle

DOM event handlers use `wasm_bindgen::Closure`. These must stay alive
as long as their DOM elements exist. The DomRenderer stores all
closures in a `ClosureVec` (Rc<RefCell<Vec<JsValue>>>).

On each rebuild:
1. Clear old DOM elements (detaches them from the tree)
2. Clear the closure vec (drops old closures — safe because their
   DOM elements are gone)
3. Rebuild DOM and create new closures (stored in the closure vec)

This avoids `Closure::forget()` which leaks memory permanently.
Each window's `render_dom()` receives a `DomCtx` with the shared
closure vec, so all closures from all windows are managed together.

### DomCtx event helpers

`DomCtx` bundles action queue, repaint signal, closure storage, and
window ID. Views use its helpers:

```rust
// Static WindowEvent (button, radio, checkbox):
ctx.on_window_event(&btn, "click", "event_name", "value");

// <select> change -> WindowEvent with selected value:
ctx.on_select_change(&select, "event_name");

// Any specific Action:
ctx.on_action(&btn, "click", Action::ClearEventLog);

// Complex handler (reads DOM, conditional logic):
ctx.listen(&btn, "click", |e| { /* custom logic */ });
```

### DOM theme

Style constants in `src/dom/theme.rs`: `BTN_PRIMARY`, `INPUT`,
`LABEL`, `SELECT`, `PRE_OUTPUT`, `SECTION`, `HEADING`,
`SECTION_GROUP`, `LABEL_CHOICE`, etc.

### Action flow

```
DOM event handler
  |-- pushes Action to pending_actions (Rc<RefCell<Vec<Action>>>)
  |-- calls repaint()
  |
  v
DomRenderer::render() (next frame)
  |-- drains pending_actions into actions vec
  |-- checks snapshot, rebuilds DOM if changed
  |
  v
EntityApp::process_actions()
  |-- SpawnWindow / CloseWindow: window lifecycle
  |-- Navigate / NavigateUp: dispatched to target window's handle_action
  |-- WindowEvent: dispatched to target window's handle_action
  |-- ConnectPeer / StartListener / Execute: async, spawned on runtime
  |-- ClearEventLog: clears shared event log
```

Sync actions (window ops) are handled immediately. Async actions
(peer operations) are spawned on tokio (native) or spawn_local (WASM).
Results go to `PeerManager.event_log`, visible in Event Log window.

## Window Types

| Window | State | Entity Type | State Scope |
|--------|-------|-------------|-------------|
| Entity Tree | peer_id, current_path | `app/state/entity_tree` | Per-window |
| Event Log | (reads shared event_log) | — | Shared (Rust vec) |
| Execute Console | mode, selected_peer, handler, operation, resource, raw fields | `app/state/execute_console` | Per-window |
| Peer Connections | address input | `app/state/peer_connections` | Per-window |
| Query Console | type/path/ref filters, limit, include_entities | `app/state/query_console` | Per-window |
| Settings | theme, auto_connect, show_inspector | `app/state/setting` | Global |
| Key Manager | (placeholder) | — | — |
| Knowledge Base | mode, scope selection, drafts | `app/state/knowledge_base` | Per-window |
| Peers (management) | (derived from PeerManager) | — | Stateless |

### Entity Tree

Three-panel layout: tree nav, document content, inspector metadata.
Each panel is a separate DOM module (`dom/tree.rs`, `dom/document.rs`,
`dom/inspector.rs`). All receive peer_id and current_path from the
window's entity-backed state.

### Event Log

Reads from `PeerManager.event_log` (shared `Arc<Mutex<Vec<String>>>`).
No entity-backed state — the log is ephemeral and session-scoped.
Has a clear button that dispatches `Action::ClearEventLog`.

### Execute Console

Two modes: guided (select handler/operation from dropdowns) and raw
(type handler URI and operation directly). Peer selector for
targeting local vs remote handlers. Results go to the shared
event log.

### Peer Connections

Address input for WebSocket connections. QR code display showing
the native peer's listen address (via `ws_listen_addr`). Camera
scanner for QR pairing (`dom/scanner.rs` with BarcodeDetector).
Shows connected peers list.

### Query Console

Queries the `system/query` handler with filters: entity type, path
prefix, reference, path pattern. Configurable limit and
include_entities toggle. Results displayed as formatted CBOR.

### Settings

Global application preferences. Theme (dark/light), show inspector
toggle, auto-connect toggle. All Settings windows share the same
tree path. `ensure_state` pattern prevents second window from
overwriting changes made in the first.

### Key Manager

Placeholder. Will become the identity management hub: user identity,
capability tokens, login/logout, identity import/export.

## Window Concept Evolution

### Entity Tree (current)

**Purpose:** Navigate and inspect the raw entity tree structure.
Structural tool — works for any entity at any path regardless of type.

### Entity Browser (future)

**Purpose:** Type-aware content rendering. Would add: type renderer
registry, content domain conventions, cross-entity navigation via
entity URIs. Semantic tool — interprets entities through their types.

The tree inspector is infrastructure that feeds the content browser.
Both are projections of the same entity tree with different scope.

## Architectural Direction

### Model extraction (active direction)

Currently each view file combines state, mutation logic, and rendering.
The Go workbench separates these into a model layer (renderer-neutral
output structs) and thin renderers. The same separation now applies
here for two reasons:

1. **Cross-renderer portability is becoming load-bearing.** This
   project may add a native pixel-canvas target back as a
   first-class target, may
   gain a Tauri-native backend panel, and may share UI logic with
   the Godot Studio project via entity types. None of those is
   tractable while business logic is fused with DOM construction.

2. **The UI convergence pattern is validated.** Five UI projects
   across the workspace independently converged on the same five
   views (tree / detail / execute / log / workspace). The shape is
   stable enough to be projected through a model layer instead of
   re-implemented per renderer. The Go workbench's
   console + canvas projects already share models — that's the
   target architecture.

**Current:** `render_dom()` reads tree + formats data + builds DOM
**Target:** `model.render()` → output struct → `render_dom()` builds DOM

**Greenfield-first migration strategy.** We are not refactoring
existing windows preemptively. Instead:

- **New windows** (starting with the knowledge base wiki) adopt
  the model pattern from day one. They become the reference
  implementation.
- **Existing windows** migrate when they're touched anyway, or in
  focused batches once we've validated the pattern through one or
  two greenfield builds.
- **Discovery before refactor.** What the model layer actually
  needs (output types, ValueKind variants, lifecycle hooks) is
  best learned by writing one window in the new pattern and
  letting the design pressure surface, then applying that to
  existing windows with confidence.

Key enabler: **ValueKind semantic classification** in `format.rs`.
Every formatted text piece carries its meaning (Hash, Path, String,
Number, Error) so renderers can apply styling, click handling, and
navigation without business logic. Go's `FormattedValue` with `ValueKind`
is the pattern.

The per-window comparison and effort estimates drive which windows
migrate first.

### Multi-peer window binding

Currently all windows bind to `primary_peer_id()` at spawn time.
With multi-peer (see `ENTITY-SDK-API.md`),
windows will bind to any local peer. The window struct already
stores `peer_id` — the change is making the factory accept a
target peer_id instead of always using primary.

### State ownership split

With the peer identity model, window
state ownership splits:
- **Session state** (open windows, layout) → app peer's tree
- **User preferences** (theme, inspector) → user peer's tree
- The Settings window will need to read from the correct peer
  based on the setting's ownership

### Command registry (future)

The current `Action` enum is flat. Evolution toward discoverable
commands bindable to keyboard shortcuts and available in contextual
menus.

### Targeted refresh (future)

Currently: full DOM rebuild on any change. Target: subscription-based
path filtering so windows only rebuild when their subscribed paths
change. The generation counter is a global trigger — per-window
subscriptions would be more precise.

### Type renderer registry (future)

Map entity types to render functions with a degradation hierarchy:
1. Type-specific renderer (e.g., `doc/paper` → formatted markdown)
2. Type-definition structural (read the type definition, display fields)
3. Extends chain fallback (follow `extends` to a known base type)
4. CBOR diagnostic (raw entity display — current state)

## How to Add a New Window

See `../guides/DEVELOPER-GUIDE.md` for the step-by-step guide. Summary:

1. Create `src/views/my_window.rs` with state struct + WindowView impl
2. Register module in `src/views/mod.rs`
3. Register type in `src/app.rs` (both WASM and native paths)
4. Define state struct with `from_entity()` / `to_entity()` for CBOR
5. Implement `read_state()` / `write_state()` using PeerManager + SDK paths
6. Implement `render_dom()` using DomCtx helpers and theme constants
7. Implement `handle_action()` — read state, modify, write back
8. Factory writes initial state to tree at spawn time

## Source Files

| File | LOC | Purpose |
|------|-----|---------|
| `window.rs` | 318 | WindowView trait, WindowManager, WindowId, RepaintFn, ClosureVec |
| `action.rs` | 88 | Action enum (no `RenderMode` — removed with the egui path) |
| `app.rs` | 542 | EntityApp: process_actions, async handlers, frame loop |
| `dom/mod.rs` | 290 | DomRenderer: Shadow DOM, per-window dirty rebuild (subscription-driven), command palette |
| `dom/util.rs` | 160 | DomCtx, DOM helpers, event listener management |
| `dom/theme.rs` | 66 | Inline style constants |
| `dom/style.rs` | 451 | CSS for Shadow DOM (responsive) |
| `dom/tree.rs` | 159 | Tree navigation panel |
| `dom/document.rs` | 65 | Document content panel |
| `dom/inspector.rs` | 70 | Entity inspector panel |
| `dom/scanner.rs` | 465 | QR camera scanner |
| `views/entity_tree.rs` | 326 | Entity Tree window |
| `views/execute_console.rs` | 421 | Execute Console window |
| `views/query_console.rs` | 587 | Query Console window |
| `views/settings.rs` | 394 | Settings window |
| `views/peer_connections.rs` | 289 | Peer Connections window |
| `views/event_log.rs` | 102 | Event Log window |
| `views/key_manager.rs` | 100 | Key Manager placeholder |

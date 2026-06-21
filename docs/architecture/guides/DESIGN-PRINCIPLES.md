# Design Principles — Lessons from HyperCard, Decker, Emacs

**Source**: `entity-core-papers/papers/09-application-architectures/notes/hypercard-decker-emacs-exploration.md`

Principles derived from the architectural exploration of application
environments where using and building are the same activity. These
inform how the Entity Browser should work, not just what it renders.

## 1. One Substance, Visible, Navigable

> "Everything is a buffer" (Emacs). "Widgets ARE the content store" (Decker).

**For us**: Everything is an entity. The Entity Browser should treat ALL
state as entities in the tree — not just user data, but:

- Window layouts and positions
- Configuration and preferences
- Handler registrations
- Peer connections and capabilities
- The browser's own state

If it's state, it's an entity. If it's an entity, it's browsable,
inspectable, and navigable through the same interface as user data.

**Current gap**: We have per-window state (current_path) that lives in
Rust structs, not in the entity tree. Settings are Rust fields, not
entities. Peer connection status is a Rust vec, not tree entries.

**Direction**: Entity integration (Phase 5) should move application
state into the entity tree. Window state at `system/browser/windows/*`,
config at `system/browser/config/*`, peers at `system/peer/*`. The
Entity Browser then browses itself.

## 2. Progressive Disclosure

> HyperCard's five levels: Browsing → Typing → Painting → Authoring → Scripting.

**For us**: Users enter by browsing entities. They should be able to go
deeper without switching tools:

| Level | Capability | Entity Browser Feature |
|-------|-----------|----------------------|
| 1. Browse | Navigate tree, read content | Tree panel, document viewer |
| 2. Inspect | See types, hashes, metadata | Inspector panel |
| 3. Query | Search and filter entities | Query bar (planned) |
| 4. Execute | Run operations on handlers | Execute Console |
| 5. Modify | Edit entities, register handlers | Entity editor (planned) |

The same environment at every level. No "developer mode" switch. Just
increasing capability as the user explores deeper.

**Current state**: Levels 1-2 are working. Level 4 is a placeholder
window. Levels 3 and 5 need entity integration.

**Maps to entity capability model**: The progressive disclosure levels
map to the protocol's capability tiers. A browser peer might start with
read-only access (levels 1-2), gain query capability (level 3), then
execute capability (level 4), then write capability (level 5).

## 3. Stateless Handlers on Visible State

> "Variables don't persist between events. Only widget state persists." (Decker)

**For us**: Handlers read tree state, transform, emit. No hidden handler
state. The entity tree IS the state. Handlers are pure functions from
tree state to tree mutations.

**Implications for the browser**:
- Entity Browser should show what handlers see — the tree, the types,
  the capabilities. Not a processed/transformed view.
- Execute Console should show the raw EXECUTE request and response,
  not a prettified summary.
- Clicking a tree item is an operation: `system/tree get {path}`.
  The browser should make this visible — you're not "navigating a UI",
  you're executing entity protocol operations and viewing the results.

**Implication for render_dom**: The view method should be as stateless as
possible. Read from the entity tree, produce DOM. Per-window state
(current_path) is the minimal exception — and ideally even that should be
entity-backed (it is today).

## 4. Documents = Applications

> A stack IS the application. A deck IS the application. Copy, share, run.

**For us**: An entity subtree with types + handlers + data is a complete
thing. Snapshot it, sync it, reconstruct it on another peer.

**Implication for the browser**: The WASM build + seed data is already
close to this — a single `dist/` directory that IS the application with
its data. The architecture doc's "computational genome" concept.

**Future**: A workspace entity at `workspace/my-project/` contains
type definitions, handler configurations, data entities, and UI layout
preferences. Sync that subtree to another peer → same workspace appears.
The Entity Browser running on that peer renders it the same way.

## 5. Query as First-Class

> Lil builds SQL-like queries into the language. "extract all cards where visited = true."

**For us**: The entity tree is a database. The browser needs query to be
first-class, not an afterthought.

**Planned features**:
- Query bar in the Entity Browser (filter tree listing)
- Cross-path queries ("all entities of type document/markdown")
- Query results as navigable entity lists
- Save queries as entities themselves (query entity → result set)

## 6. Self-Modification Through the Same Interface

> "M-x describe-function shows the source. You can advise any function." (Emacs)

**For us**: The Entity Browser should be able to inspect and modify:
- Its own type definitions (at `system/type/*`)
- Its handler registrations (at `system/handler/*`)
- Its capability grants (at `system/capability/*`)
- Its peer configuration (at `system/peer/*`)

Through the same tree navigation, inspection, and editing interface used
for user data. No separate admin panel — it's all entities.

## 6a. Accessibility Is Structure

> "DOM mode works better than canvas because DOM IS the structure."

This principle emerged from this project's dual-renderer experiment
and is now reflected in the entity system's broader UI research.
**Accessibility is not a layer added on top of rendering — it is a
property of how close the rendering medium is to a structural
representation.**

The DOM is accessible because:
- Screen readers consume DOM directly (no extra hooks)
- Browser navigation (Ctrl+F, tab order, scroll, focus management)
  works for free
- Form semantics (labelled inputs, role attributes, ARIA where
  needed) are first-class
- Mobile gestures, responsive layout, text reflow — all native
- The structure of the page IS what users navigate

An immediate-mode canvas renderer, by contrast, requires a separate
accessibility layer (e.g. AccessKit) on top to provide the same
affordances. The accessibility is added back, not inherent.

**Application to the entity system**: The closer a renderer
projects entities into structural form, the more accessibility
emerges as a property of that projection. A type renderer registry
that maps `doc/paper` → semantic HTML (headings, paragraphs, lists)
preserves accessibility for free. A type renderer that draws
`doc/paper` as a canvas of glyphs has to add accessibility back.

This is one reason DOM became the primary rendering target rather
than just an accessibility shadow. It's also why the type renderer
registry direction matters: it's the bridge from generic DOM to
type-aware DOM that stays structural.

**Generalization**: Wherever we have a choice between "rendering
that produces structure" and "rendering that produces pixels,"
prefer the structural option unless there's a specific reason to
go to pixels. Pixels are an output target; structure is also a
specification, an interface, and an accessibility tree.

## 7. How This Applies Across Implementations

The entity system has three GUI applications. They share the protocol,
not the UI. But the **principles** should be consistent:

| Principle | Entity Browser (Rust DOM) | Entity Studio (Godot) | Entity Workbench (Go) |
|-----------|--------------------------|----------------------|----------------------|
| One substance | Entities in DOM tree | Entities in scene tree | Entities in debug views |
| Progressive disclosure | Levels 1-5 in browser windows | Workspace privilege levels | Developer-focused (levels 3-5) |
| Visible state | Tree/inspector panels | Panel state = entity state | Protocol message inspector |
| Documents = apps | WASM + seed data bundle | Workspace as .tres/.scene | N/A (developer tool) |
| Query | Query bar (planned) | Command palette queries | REPL-style execute |
| Self-modification | Browse system/ entities | Edit panel configs in Godot | Modify handlers at runtime |

**What transfers between implementations**: The entity tree structure,
type definitions, handler registrations, and capability grants are
protocol-level. Any peer can read them. A type defined in Godot Studio
appears in the Entity Browser's inspector. A handler registered by the
Go Workbench is executable from the Rust browser. The protocol is the
transfer mechanism — not shared UI code.

**What doesn't transfer**: UI layout, panel arrangement, rendering
approach, widget implementations. Each framework does these its own way.
That's fine — the entity system's value is in the data and protocol
layer, not the UI layer.

## 8. Summary: What Entity Integration Should Prioritize

Based on these principles, Phase 5 (entity integration) should focus on:

1. **Entity-backed state** — move browser state into the tree so it's
   self-inspectable
2. **Operations as entities** — browsing is executing operations, make
   this visible
3. **Type-aware rendering** — entity type determines how it renders in
   the document panel (markdown → HTML, system/type → type inspector, etc.)
4. **Query** — filter/search over the tree, results as navigable lists
5. **Progressive capability** — the browser adapts to what the connected
   peer allows (read-only → query → execute → write)

The rendering infrastructure (DOM, windows, actions) is ready. The next
step is making the entities themselves drive the experience.

## 9. Alignment with Godot Entity Studio Vision

The Godot SYSTEM-VISION.md (`godot-entity-core-rust/docs/SYSTEM-VISION.md`)
has a more developed model for several areas. Key takeaways for the Entity
Browser:

### Panel Communication via Entity Tree

Godot's model: panels don't talk to each other directly. They read/write
to conventional entity paths. Other panels watch those paths via
tree_changed signals.

```
Tree panel selects "user/notes/a"
  → writes to: system/ui/selection = "user/notes/a"
Inspector panel watches system/ui/selection
  → loads entity at that path
  → displays type, data, hash
```

**For the Entity Browser**: Our current Action dispatch model (Navigate,
NavigateUp) is similar but less entity-native. In the entity-integrated
version, clicking a tree item should write to `system/ui/selection` in
the entity tree, and the document/inspector panels should react to tree
changes at that path. Same pattern, backed by real entities.

### Multi-Peer Architecture

Godot separates the App Peer (local UI state, no networking) from Network
Peers (syncing, replication). The App Peer is always present and owns:
- `system/ui/selection` — current selection
- `system/ui/workspaces/` — window layouts
- `system/ui/panels/*/state` — per-panel state

Network Peers own user data and handle sync.

**For the Entity Browser**: We should adopt the same split. The browser's
EntityState becomes the App Peer — local, fast, synchronous reads. When
we add networking (Phase 7), connected peers are separate EntityPeer
instances. The browser reads UI state from the app peer (no latency) and
user data from network peers (async, may lag).

This means `EntityState` as currently implemented is essentially the app
peer's storage. The WindowView's per-instance state (current_path, etc.)
would move to `system/ui/panels/{window_id}/state` in the app peer tree.

### Panel Contract

Godot's panels receive context via `set_context()` — peer references,
framework services, slot identity. Our `WindowView` trait is similar but
less structured (DOM-only — there is a single render path):
- `render_dom(&self, container, state, ctx: &DomCtx)` — the single render path
- `handle_action(&mut self, action, peers: &Peers)` — targeted action handling

For entity integration, we should move toward a `WindowContext` struct:

```rust
struct WindowContext<'a> {
    app_peer: &'a EntityState,       // local UI state
    network_peers: &'a [EntityPeer], // connected peers (future)
    window_id: WindowId,
    actions: &'a mut Vec<Action>,
}
```

This mirrors Godot's `set_context(context: Dictionary)` pattern.

### Command Surface

Godot has a command registry with two interfaces (visual palette + text
terminal). Our Action enum + command palette sidebar is the visual
interface. The Execute Console window is the text interface. Both should
dispatch to the same command registry.

For entity integration: commands become entity operations. The command
palette lists available handlers and their operations. The Execute Console
lets you compose raw EXECUTE requests. Both produce the same Actions.

### Progressive Disclosure (Aligned)

Both projects implement the same four-level model:

| Level | Godot | Entity Browser |
|-------|-------|---------------|
| Browse | Normal mode | Entity Browser window |
| Inspect | Panel interaction | Inspector panel |
| Command | Command palette | Execute Console |
| System | System terminal | (planned) Full tree editor |

### What Transfers via Protocol

The entity tree, type definitions, handler registrations, and capability
grants are protocol-level data. A type defined in Godot Studio appears in
the Entity Browser. A workspace layout saved as entities in one application
can be loaded by another.

What does NOT transfer: UI framework code, rendering logic, panel
implementations. Each application builds its own UI from entity data.
The protocol is the interop boundary.

### Key Differences (By Design)

| Aspect | Godot Studio | Entity Browser |
|--------|-------------|---------------|
| Framework | Godot scene tree | HTML DOM (web-sys) |
| Panels | GDScript scenes in slots | WindowView trait impls |
| Layout | Tiled workspace layers | Floating windows / stacked sections |
| Scripting | GDScript + Lil (future) | Rust only |
| Peer binding | Per-panel context | Shared EntityState |
| Rendering | Godot renderer | DOM only |
| Target | Desktop (Godot export) | Web browser, Tauri WebView |

These differences are intentional — each framework plays to its strengths.
The entity protocol unifies them at the data layer.

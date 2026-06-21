# System Vision — entity-browser-rust

**Status**: Living document. Updates rare — this is the doc that
changes when the strategic direction changes, not when
implementation evolves.
**Purpose**: Cohere the long-running architectural ideas that don't
fit cleanly in any one spec. The other docs answer "how is this
built" and "what's next." This one answers "what is this for and
where is it going."

---

## What This Is

A full peer client for the entity system, written in Rust, building
to native desktop, web (via WASM), Tauri desktop, and (eventually)
mobile native shells from a single codebase. Real entity protocol,
real multi-peer state, real handler dispatch — not a thin client
talking to a server.

The browser tab is a full peer. The native desktop is a full peer.
Tauri is a full peer in a WebView with an optional native backend
peer alongside it. None of these is a degraded version of the
others. They share the same protocol, the same data model, the
same UI code where possible. What differs across deployment modes
is which transports are available and whether the peer can listen
for incoming connections.

This project is the primary client through which a user is most
likely to first encounter the entity system. The other UI projects
in the workspace serve specialized roles — Godot Studio for full
authoring, Workbench for developer tooling, raylib visualizer for
graphical experiments. This one is the user-facing entry point and
**the most plausible primary peer manager for an end user**.

That positioning matters. Architectural decisions should weight
what the system needs to be when it's the main interface a user
sees, not when it's one experiment among many.

---

## The Bigger Picture: A Shared UI Substrate

This project is not building a UI toolkit alone. The teams across
the workspace — Rust (this project, Godot Studio), Go (workbench
console + canvas), and others — are jointly building a
**cross-platform UI substrate** for the entity system.

The substrate has two halves:

**1. The protocol layer.** Entity types, handler operations,
capability grants, sync, content addressing, continuation chains,
compute expressions. This is the language all peers speak. UI
state, layout, interaction descriptions — all expressible as
entities. UI definitions are content-addressed, transferable
across peers, runnable on any peer with a compatible renderer.

**2. The renderer layer.** Each implementation (Rust DOM, Godot
scenes, Go raylib, Go tview, future ones) implements the type
renderers and layout primitives in its native medium. The renderer
is platform-specific; the program (the entity subtree describing
what to render) is portable.

The convergence point is **types, not code**. A type defined and
populated by one peer can be read and rendered by any other peer
with the right renderers installed. Compute handlers transfer.
Subscription patterns transfer. Window state transfers. UI
definitions transfer.

This is the HyperCard / Decker / Smalltalk lineage made network-
native: one substance (entities), many projections (renderers),
all sharing the same content-addressed structural representation.
Independently, the Go and Rust teams built separate UIs — and they
converged on the same five views (tree, detail, execute, log,
workspace) without coordination. The shape is being dictated by
the primitives, not the frameworks. That convergence is the
evidence the substrate is real.

This project's role in the substrate is to be the most
multi-platform deployment target — same code on web, native, Tauri,
and mobile — and to validate the SDK in Rust as the substrate's
main systems language. As the SDK matures (Layer 2 algorithms,
pipeline builder, type renderer registry, eventually entity-native
UI subtrees), this project is one of the first consumers and one
of the first reference implementations.

---

## Architectural Pillars

These are the decisions that have stabilized. They don't change
casually.

### DOM-Primary Rendering

DOM is the primary rendering target. It started as an accessibility
shadow alongside a native canvas renderer; testing revealed it was
the better UX for most interaction types and the most portable across
platforms.

The deeper reason is that **DOM IS the structure**. Screen readers,
browser navigation, responsive CSS, mobile gestures, form
semantics — none of these are added on top of DOM rendering.
They're properties of being the DOM. A pixel canvas renderer requires
an accessibility layer on top to provide the same affordances. The
closer a rendering medium is to a structural representation, the more
accessibility is inherent rather than added.

This generalizes to a principle: where there's a choice between
"rendering that produces structure" and "rendering that produces
pixels," prefer structure unless there's a specific reason for
pixels. See `DESIGN-PRINCIPLES.md` section 6a.

DOM is the only rendering path today; the earlier native canvas
(egui/eframe) renderer has been removed. Whether a pixel-rendering
path returns later (a native canvas target, or embedded canvases for
real-time visualization) is an open question (see the open questions
below); for now, "rendering that produces structure" won, and DOM
leads unopposed.

### Browser Is a Full Peer

This is a deployment architecture pillar with implications for
everything else. The WASM build is the same code that runs natively,
compiled to a different target. It runs the same protocol, manages
the same tree, executes the same handlers, evaluates the same
compute expressions. A browser peer and a native peer are the same
peer at different targets.

What the browser cannot do (listen for inbound connections, run as
a daemon, access TCP, access the filesystem) is a property of the
browser sandbox, not an absence of P (the peer primitive). The
browser peer is a full participant whose P capabilities are
"active outbound participant" rather than "passive server." It can
join any network where at least one peer is willing to accept
inbound connections.

The browser is the **primary management interface**. The native
peer is the **persistent participant**. Together they cover the
deployment surface. A typical user might run a browser peer on
their laptop (UI, inspection), a native peer on the same laptop
(storage, network, keys), and connect to other peers on other
devices. All speak the same protocol; none is privileged.

See `GUIDE-DEPLOYMENT-AND-CONFIGURATION.md` for the concrete
deployment modes.

### Entity-Backed State

All UI state — window layouts, current paths, settings, draft
content, selection — lives as entities in the peer's tree. Not in
Rust struct fields, not in parallel data structures, not in a
sidecar config file. The tree IS the state.

This is already implemented for all current windows under the
application namespace `{peer_id}/app/entity-browser/` (per the SDK
domain spec). State changes increment the SDK generation counter,
signalling "something changed" so the DOM rebuilds. State persists
across sessions when the underlying store does.

The principle is simple and load-bearing: **if it's state, it's an
entity**. This is what makes the application self-inspectable
through itself, what allows the cross-team substrate to read this
project's UI state, and what makes synchronizing UI state across
devices a property we get for free rather than a feature we have
to build.

### Multi-Peer Architecture (Lightweight Peers)

The PeerManager holds multiple peers in one process. Each has its
own identity, its own tree, its own handler registry. They
communicate via the same protocol they would use across a network.
A single browser tab can be a multi-peer network.

Peers in the entity system are lightweight enough to be the unit
of isolation, scope, identity, execution, and distribution all at
once. This is closer to Erlang processes than to Urbit planets or
Holochain conductors. Heavy peer designs require internal scope
hierarchies (facets, dataspaces, zomes) to compensate; lightweight
peers don't. We get fate-sharing, isolation, and coordination from
the same primitive.

Practical use: app peer for UI state, user peer for identity,
backend peer for native infrastructure, managed peers for
collaborators. Each plays a role; the protocol mediates between
them. Adding more peers is cheap.

### Protocol Boundary, Made Visible

Application code talks to peers through two named surfaces (see
`ENTITY-SDK-API.md` for the L0/L1 operations split):

- **L1** — `ctx.get/put/list/remove/has` (async, dispatched through
  `peer.execute("system/tree", ...)`, capability-checked). This is the
  default.
- **L0** — `ctx.store().get/put/...` (sync, direct to store/index).
  An explicit escape hatch for render loops and bootstrapping; every
  call is a visible opt-out from the security boundary.

The reason the boundary is named rather than forbidden: sync render
and action loops genuinely need direct reads, and burying them behind
async would lie about what the code does. Making L0 a named call
makes every bypass grep-able and reviewable.

The long-term direction remains "L1 by default": the UI can do
anything through the protocol, and L0 exists for the narrow cases
where synchronous access is actually required. If the UI can't do
something through L1, the protocol is incomplete — forcing L1 as the
default surfaces those gaps before they become permanent, and makes
this project work identically against any peer — local, remote, or
in another language — because protocol access is what crosses peer
boundaries.

The remaining app-side step is making `handle_action` async so view
writes can migrate from L0 `store().put` to L1 `ctx.put().await`.
That shifts writes through the handler dispatch pipeline without
changing the user-visible API surface.

### Model/View Separation

Window business logic is separated from rendering through a model
layer that produces renderer-neutral output structs. The Go
workbench validated this pattern: console (tview) and canvas
(raylib) consume the same models with zero duplicated business
logic. We adopted the same shape: all windows are on the
model/output/render split.

The reason this matters is the substrate pillar above. The
renderer-neutral output struct is what makes a third rendering
surface (text/shell) viable as a peer, not a parallel codebase.
We may share window definitions with Godot Studio. None of those
is tractable while business logic is fused with DOM construction.

See `WINDOW-ARCHITECTURE.md` for the specifics of the
model/output/render split.

### Shell-First Feature Surface (Active Direction)

The entity shell is the leading edge of feature development for
peer/identity/capability surfaces — and for this project, the
primary command-surface affordance the browser peer offers to
power users. New cross-cutting capabilities ship as `shellcmd`
verbs first, presentation panels follow as thin adapters over the
same command Result. This mirrors the workbench-go project's
shell-first positioning and lets the two implementations
converge on a shared verb vocabulary as features land.

For this project specifically, the shell unlocks three things at
once: (1) a usable browser-peer command surface (`cd`/`ls`/`cat`
/`exec` etc. over the same tree the GUI windows browse); (2) a
single landing point for Stage 4 identity/role/capability
surfaces so we don't GUI-build them only to rebuild as shell
verbs later; (3) the third renderer that proves
`Result`-shaped command outputs are genuinely renderer-neutral
(alongside DOM windows and workbench-go's tview).

The cross-window interop substrate is already done: the shell is
just one more producer/consumer of the existing app-aggregate
`Selection` slot. `cd /peerA/notes` publishes a `Selection`; Entity
Tree co-orients under the no-republish rule. Action wire-shape codec
for cross-impl session replay is **deferred**.

See `../guides/SHELL.md` for the shell command surface.

### All-Rust Stack

No JavaScript, no NPM, no JS bundler. Visualizations use Rust SVG
generation or direct Canvas2D via web-sys. JS
libraries are vendored when unavoidable, never imported through
NPM. The all-Rust stack is a feature, not a limitation — it gives
us a single build pipeline, one set of dependencies to audit, and
no second language to learn or debug.

This applies to the WASM build specifically. Rust native code can
of course call C libraries through the standard mechanisms when
needed.

---

## Capability Stages

A picture of what the system has in increasing richness, without
committing dates. Each stage is meaningful on its own; the
transitions are gradual.

### Stage 1 — Multi-peer client (where we are; foundation arc closed)

- Two deployment modes: browser/WASM, Tauri desktop. (The legacy
  native canvas (eframe) UI has been removed.)
- Ten windows: Entity Tree, Event Log, Execute Console, Key
  Manager, Knowledge Base, Peer Connections, Peers (management),
  Query Console, Settings, **Shell**
- Multi-peer SDK with `PeerContext` per-peer handles, lifted to
  the shared `entity-sdk` crate at
  `../entity-core-rust/bindings/sdk/` (consumed by this app + Godot)
- L0/L1 access split: async dispatched ops by default, sync
  `store()` escape hatch for render loops
- Peer classification: Primary / Local / Remote with configuration
  flags (`has_local_context`, `deletable`, `persisted`, etc.)
- Peer persistence (native filesystem and browser localStorage)
- Real entity protocol over WebSocket; in-process `memory://`
  (native) and cross-Worker `xworker://` (browser) transports via
  `MultiConnector`
- Entity-backed window state under `app/entity-browser/` per the
  SDK domain namespace
- DOM-only rendering; reactivity is subscription-driven via
  `WindowWatch` + per-window dirty flags (no global
  snapshot/generation mechanism)
- Multi-SDK router `Peers` (`src/peers.rs`) — `Vec<Sdk>` +
  per-peer route map; primary slot is boot SDK, each
  Backend\*peer spawns a dedicated Worker SDK. The
  default-to-primary defect class is dead at the type level.
- Cold-start instrumented at ~278 ms; ~159+ native
  tests + 14 peer-integration + 1 e2e_worker all green; clippy
  clean; production-ready WASM build.
- **Entity shell** crate extracted upstream
  (`entity-core-rust/bindings/shell/`) and consumed end-to-end —
  18 verbs Tier C/E, dispatcher-tier alias resolution,
  `@alias` sigil, per-variant typed scrollback. The render adapter
  consumes `VerbOutput` directly.

### Stage 2 — Content rendering and the wiki PoC

- Knowledge base window (browse, edit, save markdown articles)
- Markdown ingest from filesystem paths
- Type renderer registry for `doc/*` and `knowledge/*` types
- Per-window subscriptions replacing the whole-tree generation
  counter (Level 4a polish)
- Model layer extracted into the wiki window first, validating
  the pattern for broader use
- Settings or another small window optionally refactored to the
  model pattern in parallel as a learning exercise

### Stage 3 — Backend peer and relay

- Tauri backend peer with full native capabilities (WebSocket
  listener, TCP, filesystem, mDNS, bundled extensions)
- WASM peer auto-connects to backend on launch
- Pipeline builder (SDK Layer 2 — first major shared algorithm)
- Backend peer setup expressed as a pipeline rather than ad-hoc
  IPC steps
- Relay handler on the backend peer routes WASM peer requests to
  TCP-only network peers
- Remote tree browsing through synced subtrees in the universal
  namespace

### Stage 4 — Capability and identity

- Real capability exchange on connect (replacing
  `debug_open_grants`)
- User identity layer separate from app peer
- Login/logout via capability token delegation
- Grant management UI in the Key Manager window
- State ownership split: app peer for session, user peer for
  preferences

### Stage 5 — Cross-renderer portability

- Type renderer registry as a first-class abstraction
- Entity-native UI subtrees: layout, view types, interactions
  expressed as entities
- A window definition built in this project loads in Godot Studio
  with comparable behavior (and vice versa)
- Compute handlers and subscription patterns shared across
  renderers via type definitions
- Mobile native shells (iOS WKWebView, Android WebView) running
  the same WASM build with native peer alongside

### Stage 6 — Self-modification

- The application can browse and edit its own type definitions,
  handler registrations, capability grants, peer configuration
  through the same interface used for user data
- Dynamic handler loading (via WASM modules or entity-native
  compute)
- Window types installable as entities, not compiled into the
  binary
- The progressive disclosure ladder (browse → inspect → query →
  execute → modify → author) is fully realized

These stages are not strict ordering. Some can happen in parallel.
Some pieces of later stages may show up early as scaffolding. The
point is the direction, not the schedule.

---

## What Coheres Here That Doesn't Fit Elsewhere

A few cross-cutting ideas that show up in multiple specs and
deserve a single home.

### The Convergence Is Diagnostic

Five UI projects in the workspace, built independently in
different languages and frameworks, converged on the same primary
views (tree / detail / execute / log / workspace). When that
happens, it means the primitives are dictating the interface
shape, not the framework choices. We're not designing the window
system from scratch — we're discovering what the entity system
wants its window system to be.

This is why we can be confident in extracting a shared model
layer (the views are stable enough), why we can talk about a
cross-team UI substrate (the convergence already happened), and
why the type renderer registry is a real abstraction rather than
a speculative one (the views map to clear type categories).

### The DSL Question Has an Answer

A recurring question across all the architecture documents: what
does a DSL for the entity system look like? The answer that
emerged from the academic team's exploration matches our own
direction: the DSL is not a language. It's syntax sugar for
constructing entity subtrees. The "compiled program" is the
entities in the tree. The same machinery (types, paths, compute,
continuations, subscriptions) works for UIs, pipelines, content
management, build systems — every domain.

This means we don't need to design a UI DSL separately from a
pipeline DSL separately from a content DSL. We design one entity
construction pattern (with possible per-domain syntax sugar) and
apply it everywhere. The progress on Layer 2 SDK algorithms is
progress on the DSL substrate, even though we don't call it a
DSL.

### The Reactive Framework Is Already Here

The combination of subscriptions, the event bridge, and the DOM
snapshot rebuild loop is already a complete reactive framework.
Subscribe to changes, get notified, re-render. The compute
extension (when it ships) adds declarative derived values on
top — but the basic reactive layer is already working today, just
at coarse-grained whole-tree granularity. The Level 4a polish
work (per-path subscriptions with typed callbacks) is a refinement
of an existing capability, not a new one.

This reframing matters because it changes how we talk about the
SDK and the framework. Reactive bindings aren't "future tier";
they're working today, just not yet exposed at the ergonomic
level the SDK should provide.

### Pipelines Are Critical for THIS Application

Continuation chains and the fluent pipeline builder are more
important for this project than they sound in the abstract. The
reason is concrete: this application depends on a native host
peer (the Tauri backend, or a standalone service) to set
up relays and subscriptions on its behalf. Relay registration,
remote subscription wiring, backend peer setup, capability
delegation — all of these are continuation-chain-shaped. Without
a pipeline builder, every relay/subscription wiring is hand-rolled
continuation entity construction, which doesn't scale and doesn't
survive the SDK boundary.

The pipeline builder is therefore not a "future SDK enhancement"
for us — it's the substrate that the connective fabric of this
application is going to depend on. See `ENTITY-SDK-API.md` Phase
5 for the concrete API direction.

### Knowledge Base as the First Application

The knowledge base wiki PoC isn't just one of many possible
applications. It's the first concrete application demonstrating
the entity system as a content management tool, the first
greenfield window in the model pattern, and a candidate for the
team's own working notes substrate over time. Markdown content as
`knowledge/article` entities, ingest from filesystem paths,
edit/save/browse cycle in the entity browser — these are all
exercises of the protocol's value proposition.

The PoC is intentionally scoped narrowly (display, edit, save,
browse — markdown rendering and term extraction deferred). Window
readiness and the editing cycle matter more than rendering polish.
What we learn from building it informs the broader model layer
extraction and the type renderer registry.

---

## Open Questions That Shape the Vision

These are the things we don't know yet that, as they get
answered, will shape where this vision moves. Listed not because
they need answers now, but because they're the live unknowns.

**1. When (or if) does a native pixel-canvas target return as a
first-class path?** The native canvas (eframe) renderer has been
removed; DOM is the only rendering path. If a future need (real-time
visualization, a native-canvas target) makes it worth bringing back,
the model layer extraction is what gives us that optionality. If DOM
continues to be sufficient on every deployment we care about, it
stays removed.

**2. What does mobile feel like in practice?** PWA via the
existing WASM build is the cheapest path. Native shells (iOS
WKWebView, Android WebView) with a bundled native peer are richer
but cost development time. We don't yet know which fits the
user's actual mobile workflows.

**3. Where does the model layer ultimately live?** This project,
extracted into a shared crate, or shared with Godot Studio
through the substrate? The greenfield-first migration buys time
to answer this empirically.

**4. How does the wiki PoC become the team's actual notes
system?** Phase 1 of the wiki design is shippable in days; the
question is whether the team adopts it for working notes,
research, design documents, meeting notes. If yes, the project
becomes self-hosting in a real way. If not, we learn what was
missing.

**5. What's the right boundary between this project and the SDK
crate we eventually extract?** The PeerManager / EntitySDK /
PeerContext layering settled the immediate question. The deeper
question — what's specific to this application versus what
belongs in a shared `entity-sdk-rust` consumed by Godot Studio
and others — we'll learn as we build.

**6. How do we expose the cross-team substrate to actual
end-users?** Not developers, end-users. What does it mean for a
non-technical user to "open a UI definition that runs anywhere"
versus "open an application built specifically for them"? The
HyperCard and Decker references answer some of this, but the
networked / capability-gated dimension is new.

These questions are not blockers. They're what we'll learn the
answers to as we build. The vision is robust enough to survive
multiple answers to each.

---

## How This Doc Relates to the Others

This document is the **stable orientation layer**. It changes
when the strategic direction changes — not when the
implementation evolves, not when we ship a feature, not when we
discover a new technical concern.

Other docs serve different roles:

- **`PROJECT-ARCHITECTURE.md`** — current architecture, deployment
  modes, module structure, design decisions. Updates as the
  architecture evolves.
- **`IMPLEMENTATION-ARCHITECTURE.md`** — runtime details, build
  pipeline, memory model, diagnostics. Updates with technical
  changes.
- **`ENTITY-SDK-API.md`** — the SDK spec, layering, abstraction
  levels, phases. Updates as the SDK matures.
- **`ENTITY-APP-FRAMEWORK.md`** — the framework layer between
  SDK and applications. Updates as that boundary shifts.
- **`WINDOW-ARCHITECTURE.md`** — the window system model.
  Updates as the model evolves.
- **`DESIGN-PRINCIPLES.md`** — durable principles from
  HyperCard / Decker / Emacs analysis. Updates rarely.
- **Reviews** (`reviews/*.md`) — analysis of specific design
  questions. Updates as analysis matures or moves to legacy.

The vision doc provides context when reading any of these. If
something in a spec or roadmap seems to contradict the vision, the
contradiction itself is information — either the vision needs to
evolve, or the spec/roadmap drifted from the strategic direction.

---

## Summary

This project is a full peer client for the entity system with the
broadest deployment surface in the workspace. It is positioned to
be the primary peer manager users may interact with, and that
positioning shapes architectural decisions toward making the
application complete enough to be the main interface for the
entity system, not one of many experimental paths.

The work is part of a larger joint effort across teams to build a
cross-platform UI substrate where entity-system content (data,
types, handlers, UI definitions, compute graphs) is portable
across renderers. The convergence of five independent UI projects
on the same primary views is the evidence this substrate is real
and the shape is being dictated by the primitives, not the
frameworks.

The architectural pillars — DOM-primary rendering, browser as full
peer, entity-backed state, multi-peer architecture, protocol-only
access, model/view separation, all-Rust stack — are stable and
support the vision. The capability stages describe where the
system grows from here without committing to dates.

Open questions remain — about a possible native pixel-canvas target,
mobile, the model layer's
home, the wiki PoC's adoption, the SDK extraction boundary, and
the end-user story for the cross-team substrate. We'll learn the
answers by building. The vision is robust enough to absorb
multiple answers.

The next concrete work: knowledge base wiki PoC built in the model
pattern, per-path subscriptions, and the pipeline builder for
backend peer setup. Each is a stage along the pillars described
above.

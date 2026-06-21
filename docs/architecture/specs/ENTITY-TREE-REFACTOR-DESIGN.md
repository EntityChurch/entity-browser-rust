# Entity Tree refactor + Workbench-pattern alignment — design + plan

**Status:** Stages A, A.1, B, C landed. Stages D–G deferred
(see §8 for stage-by-stage status). Aligns this repo with the cross-impl
workbench pattern in `GUIDE-ENTITY-WORKBENCH-APP.md` (three-impl consensus)
and the workbench-go shell direction (`SHELL-DIRECTION.md`).

## Stage status snapshot

| Stage | Scope | Status |
|---|---|---|
| **A** | Entity Tree local-mirror, flat rows, expand state | **Landed.** Test corpus 0 get_entity per render (was 381–655). |
| **A.1** | Per-event subscription via upstream `observe_with_events` | **Landed.** `ChangeOp` normalization across Direct/Worker arms. Closes the loop on `UPSTREAM-WORKER-OBSERVE-EVENT-PAYLOAD.md`. |
| **B** | Action-event metadata + per-panel + app-aggregate selection slots | **Landed.** `navigate` writes both slots. Cross-panel auto-co-orientation deliberately NOT wired pending UX clarification (see `memory/project_panel_selection_source_design.md`). |
| **C** | KB list + Event Log + Query Console + Execute Console local-mirror | **Landed.** All four windows on cache pattern; 0 get_entity / 0 tree_listing per render in steady state. |
| **D** | Exported-op factoring (panel ↔ future shell) | **Deferred.** Trigger: first real shell-verb candidate. |
| **E** | Shell surface (embedded panel + binary) | **Deferred.** Long-term; requires D. |
| **F** | Peer Connections render-time investigation | **Deferred.** Still ~7–16 ms on the §0.3 measurement; DOM-build cost, not data work. Low priority. |
| **G** | KB-as-revision-view | **Gated.** Wait on upstream revision-extension direction. |

Numbers + before/after sit in `docs/architecture/reviews/PERF-ANALYSIS.md` §0–§0.3.

## 0. Why this doc exists

The §0 perf baseline (`PERF-ANALYSIS.md`) put a real
number on the Entity Tree problem: 12.6 ms avg / 25 ms max render
with 381–655 `get_entity` calls on a 281-row corpus. The refactor
to fix it is large enough that doing it blindly would lock us into
shapes that don't match where the Go reference impl is heading.
The workbench-go team has been moving fast on patterns (action
vocabulary, selection slots, shell command extraction, panel-prefix
subscriptions) that affect what our refactor *should* look like, not
just what we need to fix.

This doc maps where we are vs. where the cross-impl convention
sits, picks the shape for our Entity Tree refactor, and stages it
so the work lands consistently with the rest of the ecosystem
rather than diverging.

---

## 1. Where the cross-impl convention sits today

### 1.1 The workbench-app convention (GUIDE-ENTITY-WORKBENCH-APP.md)

Authored from three-impl consensus (Go workbench + this
repo + Godot). Key commitments:

- **Architectural pattern**: model → renderer-neutral output struct →
  per-impl renderer. TEA-shaped. Output is plain data that crosses
  language idiom boundaries cleanly. *Already our shape.*
- **App namespace**: `app/{app-id}/...` is general-purpose. We use
  `app/entity-browser/...`; Go uses `app/workbench/...`. Both are
  valid claimed namespaces. *Already our shape.*
- **Slot table** — type names for cross-impl portable state:
  - `app/state/{content_type}` — per-content-type window state.
  - `app/state/window` — generic per-window fallback (transitional).
  - `app/state/selection` — selection slot (two-layer model, §5).
  - `app/state/setting` — global app settings.
  - `app/state/screen` — optional, for multi-screen apps.
- **Action wire shape**: `(window_id, event_name, value)` triple
  with canonical event vocabulary (`navigate`, `select`, `submit`,
  `clear`, `set_filter`, `toggle_raw`). Default propagation per
  event (some go to context, some panel-local).
- **Selection scope**: per-panel local + per-presentation-context
  propagated. Schema: `{path, paths?, peer_id?, content_type?,
  source_window?, updated_at}`.
- **Change-detection**: three styles — counter polling,
  path-targeted observation (`store.watch(prefix)`), dispatched
  subscription. Per-panel choice.
- **CLI is one render context**, not a separate app class. Same
  state, same actions, same models — different presentation.

### 1.2 The workbench-go shell direction (SHELL-DIRECTION.md)

- **Shell-first development**: new SDK features ship as CLI commands
  first; GUI panels follow as presentation layers over the same
  command vocabulary.
- **Two vocabularies** that coexist:
  - **Action events** (`navigate`, `select`, `submit`, …) — pub-sub,
    application-agnostic, what *users do* in the UI.
  - **Commands** (`cd`, `ls`, `connect`, `mount`, `exec`, …) —
    imperative ops, application-directed, what *users type*.
- **Exported-op pattern**: each shell command splits into
  - **The op** (exported function, structured args, SDK-shaped
    result) — reusable by panels.
  - **The thin verb** (arg-parses + wraps op + projects result for
    text rendering).
  Non-shell panels call the op directly; they never fake CLI args.
- **Panel-prefix subscription is mandatory** (§8.5). Anti-pattern
  is exactly what we hit: shared state cache + per-panel filter.
  Workbench-go deleted theirs; we're catching up.

### 1.3 The relevant Go reference — `workbench/tree_model.go`

The Go workbench's TreeBrowserModel is the **direct reference** for
our Entity Tree refactor. Shape:

```go
type TreeBrowserModel struct {
    peerCtx *PeerContext
    Root        *TreeNode             // tree graph w/ expand state
    VisibleRows []VisibleRow          // currently-visible rows
    known       map[string]LocationEntry  // path → entry mirror
    dirty       bool
    cancel      func()                // OnPrefixChange teardown
    // ...
}

func NewTreeBrowserModel(peerCtx *PeerContext) *TreeBrowserModel {
    m := &TreeBrowserModel{...}
    m.cancel = peerCtx.Store().OnPrefixChange("", m.onEvent)
    return m
}

func (m *TreeBrowserModel) onEvent(ev ChangeEvent) {
    // O(depth) per event — InsertOrUpdate(m.Root, entry) or Remove
    // No full-tree rescan
}

func (m *TreeBrowserModel) Render() TreeBrowserOutput {
    rows := flatten visible nodes from m.Root
    return TreeBrowserOutput{Rows, SearchText, MatchCount}
}
```

Performance characteristics from the workbench team after this
refactor: tree-browser refresh on no-event = **406 ns**
(vs. ~10 ms before, **~25,000×**).

---

## 2. Where we already align

We've been building the right shape on most axes without
explicitly knowing it. Status today:

| Convention | This repo | Status |
|---|---|---|
| Model → output → renderer | All 9 windows: `model.rs` / `output.rs` / `mod.rs::render_dom` | ✅ Aligned |
| `app/{app-id}/...` namespace | `app_paths.rs` parameterizes `APP_ID = "entity-browser"` | ✅ Aligned |
| Per-window state at `workspace/windows/{id}/state` | All windows | ✅ Aligned |
| Settings at `settings/{key}` | Settings + KB ui state | ✅ Aligned |
| Bundled CBOR state per window | All windows use `to_entity()/from_entity()` | ✅ Aligned |
| `app/state/{content_type}` type names | Settings uses `app/state/setting`; Entity Tree uses `app/state/entity_tree` | ✅ Aligned (custom content-types named locally) |
| Per-panel subscription (panel-prefix) | 8 of 9 windows via `WindowWatch.subscribe_prefix` | ✅ Aligned (mostly — see §3) |
| Per-window isolation | `WindowWatch` owns an `Arc<AtomicBool>` dirty flag | ✅ Aligned |

## 3. Where we diverge / gap matrix

Ranked by impact:

| Gap | Where | Impact | Fix lands in |
|---|---|---|---|
| **Entity Tree: full-tree scan + per-node Get on every render** | `views/entity_tree/model.rs:176-205` | 12.6 ms / 25 ms render — exceeds frame budget; §2.8 anti-pattern | **Stage A** (this work) |
| **Entity Tree: no expand state, no progressive disclosure** | Same; tree always renders fully | Unscalable past a few thousand paths; user-flagged | **Stage A** |
| **Entity Tree: inspector + document re-Get every render** | `build_inspector`, `build_document` | 2 extra Gets per render, gated only by the broad window dirty flag | **Stage A** |
| **Selection: not propagated to a tree slot** | Entity Tree stores `current_path` in window state only | No cross-window co-orientation; doesn't match guide §5 | **Stage B** |
| **No action-event vocabulary** | `action.rs` has app-specific actions but no canonical `navigate`/`select`/etc. | Diverges from guide §6; blocks cross-impl portability of action history | **Stage B** |
| **KB list: same scan-and-filter shape** | `views/knowledge_base/model.rs:252` | O(K) per render; same fix shape | **Stage C** |
| **Event Log / Query Console: full LOG_CAP scan on every render** | `event_log_writer.rs:124` + `views/query_console/model.rs:268` | Sub-ms today (small N); 30× worse at full LOG_CAP=1000 | **Stage C** |
| **No exported-op layer for action handlers** | All action handling lives inside view modules | Blocks future "shell command calls the same op as the panel" | **Stage D (deferred)** |
| **No shell/REPL surface** | We have no shell window or binary | Long-term direction; not blocking | **Stage E (deferred)** |
| **Window-id vs panel-id terminology** | We use `window_id`; Go renamed to `panel_id` | Cosmetic; cross-impl portability of selection schema | **Stage B** |
| **`Peer Connections` slow (8.8 ms / 13 ms with 0 data work)** | Surprised us in §0 baseline | DOM-build cost; not data | **Stage F (later)** |
| **`Knowledge Base` window architecture under question** | Guide §11.1: KB may become a view over the revision extension | Future scope; don't refactor twice | **Stage G (gated)** |

## 4. The Entity Tree refactor — design

### 4.1 Target shape (Rust port of the Go pattern)

```rust
pub struct EntityTreeModel {
    window_id: WindowId,
    peer_id: String,

    // Local mirror, maintained incrementally by the subscription
    // callback. Single source of truth for the rendered tree.
    inner: Mutex<EntityTreeInner>,
}

struct EntityTreeInner {
    // path → hash mirror. Authoritative; render reads from this.
    known: HashMap<String, Hash>,

    // The tree graph with expand state. Built incrementally from
    // `known`; nodes carry `expanded: bool`.
    root: TreeNode,

    // Currently-flattened visible rows. Rebuilt on dirty.
    visible: Vec<VisibleRow>,

    // Per-panel selection (cursor in the tree).
    current_path: Option<String>,

    // Cached entity for the selected path. Updated by a second,
    // narrow subscription bound to the current_path. Avoids the
    // per-render get_entity for inspector/document.
    selected_entity: Option<Entity>,

    // Search filter — Stage A2.
    search: String,

    // Internal dirty tracking. Reset by the renderer.
    needs_rebuild: bool,
}
```

### 4.2 Subscription topology

Two subscriptions per Entity Tree window:

1. **Tree mirror subscription** — `on_prefix_change_seeded("/{peer_id}/", on_event)`
   - Seed phase: synthetic Put per existing path; we `InsertOrUpdate`
     the local tree.
   - Live phase: same handler, real events.
   - Handler is idempotent: "set my view of path P to hash H."
   - Costs O(depth) per event; O(N) once at seed.

2. **Selected-entity subscription** — created when `current_path`
   changes; cancelled when it changes again.
   - Watches the exact path.
   - On Change, fetches + decodes once, stores `selected_entity`.
   - Inspector + document render from `selected_entity`, not by
     re-`get_entity`.

This is the workbench memo's "two subscriptions for inspector-style
panels" pattern (FEEDBACK-CROSS-IMPL-UI-PATTERNS.md
"Per-panel example (recipe)" §"For panels that display a single
selected entity").

### 4.3 Output struct (renderer-neutral)

```rust
pub struct EntityTreeOutput {
    pub peer_label: String,
    pub current_path: Option<String>,
    pub rows: Vec<TreeRow>,             // ← flat, not nested
    pub footer: TreeFooter,             // path_count + entity_count (O(1) now)
    pub document: DocumentView,         // from selected_entity
    pub inspector: InspectorView,       // from selected_entity
    pub search: String,                 // current search text
    pub match_count: usize,
}

pub struct TreeRow {
    pub path: String,
    pub segment: String,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
    pub has_entry: bool,
    pub leaf_count: Option<usize>,      // Some(N) on collapsed groups
    pub is_selected: bool,
}
```

Flat rows (not nested `Vec<TreeNode>`) — matches Go's
`TreeBrowserOutput.Rows`. Easier to render in any framework
(virtual scrolling, DOM, immediate-mode, …). Indentation comes
from `depth`.

### 4.4 Action vocabulary alignment (Stage B)

Adopt the canonical event names from guide §6.1 for the actions
this window produces:

| User gesture | Event (was) | Event (target) | Propagation |
|---|---|---|---|
| Click tree row | `EntityTreeClickPath` | `navigate(path)` | context |
| Click tree group toggle | (new) | `toggle_expand(path)` | panel |
| Type in search | (new) | `set_filter(query)` | panel |
| Click "go to parent" | (new) | `navigate(parent_path)` | context |

**On `toggle_expand`** — not in the guide §6.1 canonical six.
Guide explicitly says "list is not closed; content-types add
events as needed; new events should be named and registered, not
absorbed into a generic catch-all." Go review flagged this is
exactly where silent divergence happens (one impl ships
`toggle_expand`, another ships `expand_toggle` or `toggle_group`).

**Resolution**: we ship `toggle_expand` (mirrors guide's existing
`toggle_raw` shape). Propagation: panel-local. Flag to arch team
in next cross-impl pass — request a documented process for adding
canonical events. Don't block Stage A on it.

`navigate` and `select` propagate to the context-level slot
(`app/entity-browser/workspace/screens/{idx}/selection` or
`app/entity-browser/workspace/selection` for flat apps). Other
windows (Knowledge Base, Execute Console resource picker, future
inspector) can subscribe to that slot and co-orient.

Internal action enum stays Rust-idiomatic (typed enum); the wire
shape `(window_id, event_name, value)` is what gets serialized for
the per-window state and per-context selection slot. Wire <-> enum
translation happens at the persistence boundary, per guide §6.

### 4.5 Expand state — where it lives

**Local view-state on the `TreeNode`**, not persisted as separate
entities. Persisted alongside the window-state CBOR map:

```cbor
EntityTreeState {
    current_path: text?,
    search: text,
    expanded_paths: [text],   // ← new; list of paths the user has expanded
}
```

On load (`initialize` action), the model reads `expanded_paths`,
expands those nodes after the seed builds the tree. Default expand
depth of 1 (matches Go's `ExpandToDepth(root, 1)`).

Rationale: expand state is presentation, not data. Per guide §4.3,
"renderer-specific decoration lives in per-impl runtime/view
state; NOT in cross-impl `app/state/{content_type}` schema." We
keep it bundled in the window-state CBOR map because it's
persistable preference, but it's not part of the cross-impl
contract — Godot might persist expand state per-screen, Go might
not persist at all. Our schema choice doesn't bind anyone else.

### 4.6 Performance target

Hit the Go reference numbers, scaled for Rust + DOM:

- Steady-state render (no event, dirty fires from selection move):
  sub-ms.
- Subscription event with no expand change: O(depth) work in
  handler + O(visible) flatten on next render.
- Seed phase at boot: O(N) once.
- `get_entity` call count per render: **0** (inspector/document
  read from `selected_entity` cached by the second subscription).

The measurement infrastructure in place (`--features measurement`)
gives us before/after on the same e2e workload. Target:

| Window | Before (avg / max) | Gets per render | Target after |
|---|---|---|---|
| Entity Tree | 12.6 ms / 25 ms | 381–655 | < 1 ms / 0 Gets |

---

## 5. Selection propagation alignment (Stage B)

### 5.1 The two-slot model

Per guide §5 + workbench-go SHELL-DIRECTION §8.4 "Design for
item 5":

```
app/entity-browser/workspace/windows/{window_id}/selection   # per-panel
app/entity-browser/workspace/selection                       # context aggregate
```

For our current single-screen, no-multi-screen shape, the
context-aggregate path is the flat form (no `screens/{idx}/`
prefix). Future multi-screen support adds the screen layer.

### 5.2 Selection schema

`app/state/selection`:

```cbor
Selection {
    path: text?,
    peer_id: text?,          // multi-peer apps
    type: text?,             // "entity" today; future "query-result", etc.
    updated_at: uint,        // epoch ms
}
```

Workbench-go renamed `content_type` → `type` (SHELL-DIRECTION §8.4
"Design for item 5"). We adopt the cleaner name.

**On `paths[]`** — earlier draft of this doc kept `paths: [text]?`
as optional forward-compat for multi-select. **Removed**
after the Go review pointed out their Stage 5 cleanup dropped it.
Their reasoning is sound: per-panel slots make multi-select a
panel-local concern; the aggregate doesn't need a multi-select
field. If a panel ever needs multi-select, it carries `paths[]`
in its own `app/state/{content_type}` schema, not in the shared
selection slot. Pinning this aligned cross-impl before either of
us ships writers that emit it.

**Coordination ask out**: flag to arch team in next pass — pin
guide §5.4 as `{path, type, peer_id, updated_at}` only; drop
`paths[]` and `source_window`.

### 5.3 Publish + subscribe wiring

Each window decides at construction:
- **Does it publish?** Entity Tree publishes `navigate` and
  `select` to *both* its panel slot and the context aggregate.
- **Does it subscribe?** Future inspector / markdown-view panels
  subscribe to the context aggregate via `on_prefix_change`. KB
  could optionally co-orient (open the article whose path matches
  the current tree selection — gated on a future user setting).

For the Entity Tree refactor specifically, publishing is enough.
Cross-window subscription wiring lands in Stage C (KB) or later.

---

## 6. The exported-op pattern (Stage D direction; not in this work)

When our action handling grows enough that a future shell would
call the same operations, we factor ops out of the view modules.
Reference: SHELL-DIRECTION §8.2.

Today every action handler lives inside `views/{window}/mod.rs`:

```rust
// views/execute_console/mod.rs
fn handle_action(&mut self, action: Action, peers: &Peers) {
    match action {
        Action::ExecuteSubmit => { /* assembles params, calls peers.execute(...) */ }
        // ...
    }
}
```

Target shape: each operation that's a future-CLI-candidate gets
factored into a free function in a sibling `ops/` module:

```rust
// ops/execute.rs
pub struct ExecuteRequest { handler: String, op: String, resource: Option<...>, params: ... }
pub struct ExecuteResponse { /* SDK-shaped */ }

pub async fn execute(peers: &Peers, peer_id: &str, req: ExecuteRequest)
    -> Result<ExecuteResponse, ExecuteError>;
```

The view module's action handler becomes a thin parser:

```rust
Action::ExecuteSubmit => {
    let req = self.assemble_request_from_state();
    let resp = crate::ops::execute::execute(peers, &pid, req).await?;
    self.persist_response(resp);
}
```

When we later add a shell command, the shell's `cmd_exec` calls
the same `crate::ops::execute::execute` — no string-arg fakery, no
duplicate logic.

**Not in scope for this refactor.** Flagged here so we don't paint
ourselves into a corner. The Entity Tree refactor's actions
(`navigate`, `toggle_expand`, `set_filter`) don't need ops
factoring yet — they're purely tree-local. The first real op
candidates are Execute Console (`exec`) and KB (`put-article`).

---

## 7. Shell direction (Stage E; long-term, not in this work)

The user noted: "right now we don't really have a shell but the
shell has been really useful on the entity core go side… we want
to start thinking about that direction."

The convention is already clear (SHELL-DIRECTION + guide §10): a
shell would be one more render context over the same models, the
same action vocabulary, the same `app/{app-id}/...` namespace.
Concretely, when we eventually want it:

- **Embedded shell panel** — a new `views/shell/` window with text
  input + scrollback. Renders `shellcmd::Result` shapes. Submits a
  command string per Enter.
- **`shellcmd` crate (Rust)** — the canonical command vocabulary
  ported from Go's `shellcmd/` package. Same 9-core verbs (`cd`,
  `ls`, `cat`, `tree`, `exec`, …) operating on `&Peers` and
  routing to remote peers via the `entity_sdk` SDK.
- **Standalone `entity-shell` binary (Rust)** — optional, native,
  REPL form. Shares the `shellcmd` crate with the embedded panel.
- **Cross-window action interop** — `cd /peerA/notes/` in the
  shell publishes `navigate` to the context aggregate; Entity Tree
  receives the event and updates its `current_path`. Same flow as
  clicking in the tree.

**Why deferred.** Building a useful shell requires:
1. The exported-op factoring (Stage D) so commands and panels
   share logic.
2. Stable identity / capability / cross-peer-routing surfaces in
   the SDK (most are already there).
3. The shellcmd crate doesn't exist in Rust yet — Go's reference
   is ~12 files we'd port.

This is real work and earns its keep against the panel UI improvements
in Stages A–C. We don't block on it; we keep the seams clean
(Stage D) so it slots in cleanly when prioritized.

---

## 8. Phased plan

Each stage is independently shippable; landing one doesn't block
the others. Picking the order: A is the worst measured problem;
B unblocks cross-window interop without much code; C is mechanical
copies of A; D & E are deferred.

### Stage A — Entity Tree local-state refactor

**Goal:** Replace the full-scan + per-node-Get render with the
incremental-mirror + two-subscription pattern. Hit sub-ms render
on no-event, 0 Gets per render.

**Scope:**
- New `EntityTreeInner` shape (§4.1).
- Subscribe via `on_prefix_change_seeded("/{peer_id}/", on_event)`
  in `factory()`. Cancel on close.
- Selected-entity subscription bound to `current_path`, re-bound on
  selection change.
- TreeNode helpers ported from `workbench/ui_tree.go`:
  `insert_or_update`, `remove`, `flatten_visible`, `expand_ancestors`,
  `count_leaves`, `expand_to_depth`. New file: `src/views/entity_tree/tree.rs`.
- Output type: flat `Vec<TreeRow>` instead of nested `Vec<TreeNode>`.
- Renderer (`src/dom/entity_tree.rs`): consume flat rows; render
  with `depth`-based indentation; click handlers for row select +
  group toggle.
- New actions: `EntityTreeToggleExpand(path)`, `EntityTreeSetSearch(text)`.
- Persist `expanded_paths` + `search` in window-state CBOR.

**Out of scope (Stage A):**
- Selection propagation to context slot (that's Stage B).
- Action wire-shape rename to canonical `navigate` / `select`
  (also Stage B — keeps Stage A focused on render perf).

**Saturation-boundary note (from the Go team's incident):**
The tree-mirror subscription's seed phase emits N synthetic Put
events for an N-path peer. Go workbench hit a saturation/deadlock
at this exact boundary (`FEEDBACK-EVENT-DELIVERY-BACKPRESSURE.md`):
slow consumers stalled the producer, fast producers silently dropped
events on overflow.

Our upstream SDK landed the corrected pattern as part of Ask 1+2
(`ErrEventBufferFull`-style error propagation; `on_prefix_change_seeded`
uses bounded `tokio::mpsc` under the hood). The implication for
**our** callback: **keep the handler cheap**. Specifically:
- The `on_event` body acquires a `Mutex`, does an `InsertOrUpdate`
  on the tree (O(depth)), sets the dirty flag, releases. No I/O,
  no logging on the hot path, no blocking awaits.
- If the callback ever needs heavy work, push it onto a separate
  task queue; don't block the SDK delivery goroutine.
- Watch the seed phase under measurement — if 281 seed events
  produce visible callback time, scale to 14K and we'd be in the
  saturation regime.

Not a blocker for Stage A; just the discipline.

**Verification:**
- Run e2e under `--features measurement`. Compare per-window
  numbers against the §0 baseline. Target: Entity Tree
  `render_ms` < 1 / Gets = 0.
- Unit tests on the tree helpers (port `tree_model_test.go`
  selectively).

**Cost estimate:** Largest single block of work in the plan. ~1–2
days focused. Tree helpers + model refactor + DOM renderer
update + persistence schema + measurement comparison.

### Stage B — Action vocabulary + selection propagation

**Goal:** Adopt canonical action event names (`navigate`, `select`,
`submit`, `clear`, `set_filter`, `toggle_raw`) and the two-slot
selection model. Cross-window co-orientation becomes possible.

**Scope:**
- New module `src/action_event.rs`: canonical event enum with
  default-propagation metadata (mirrors Go's
  `entitysdk/action_event.go`).
- Rename current `Action` variants to canonical names where they
  map cleanly; keep app-internal actions separate.
- New slot writers: per-panel selection + context-aggregate
  selection. `Selection` struct + CBOR helpers.
- Entity Tree publishes `navigate` and `select` on row click.
- Future inspector / KB co-orient via subscription (lands in
  Stage C as KB refactor happens).

**Verification:**
- Click in Entity Tree → context-aggregate slot reflects path.
- Add e2e assertion that after clicking a tree row, the
  `app/entity-browser/workspace/selection` entity carries the
  clicked path.

**Cost estimate:** ~½ day. Mostly schema + write sites.

### Stage C — KB + Event Log / Query Console refactors

**Goal:** Apply Stage A's pattern to the remaining anti-pattern
panels.

**Scope:**
- **KB list** — local article-cache + `on_prefix_change_seeded(articles_prefix)`.
- **Event Log + Query Console** — shared local ring buffer in a
  new `event_log_cache` module (singleton or per-app-state). Both
  windows read from the cache; the cache subscribes to
  `event_log_prefix`. Either window's `read_events` becomes a
  cache read, not a tree scan.
- Optional: KB co-orients on context selection (open the article
  matching the current tree path if it's under `articles_prefix`).

**Cost estimate:** ~½–1 day. Mechanical copies of Stage A;
event-log-cache is the only new shape.

### Stage D — Exported-op pattern (deferred)

Trigger: when we add the first real "command-shaped" verb
candidate, *or* when the shell panel work starts. Could be
piggybacked on a future feature; not driven by perf.

### Stage E — Shell surface (deferred)

Trigger: user prioritization. Stage D should land first or
concurrently.

### Stage F — Peer Connections slowness investigation

Trigger: when frame budget tightens. Currently invisible because
it only fires 4 times per session.

### Stage G — KB-as-revision-view (gated by upstream)

Per guide §11.1, KB may become a view over the revision extension
rather than a top-level content-type. **Don't refactor KB twice.**
If revision-driven KB ships upstream within the next month or so,
Stage C's KB piece either waits or lands minimally (just the
prefix-scan fix). Coordinate with arch team before Stage C starts.

---

## 9. What's deferred / future-pointing

- **Multi-peer-per-app composition** (guide §9.1). We currently
  run one peer with everything in one tree (matches workbench-go's
  current shape). Future split between an "app peer" (UI state) and
  "network peers" (data) is permitted by the convention; not in
  scope.
- **Multi-screen** (guide §3.1 nested form). We're flat-window
  today. Schema is forward-compatible.
- **T3 output entities** (`app/{app-id}/ui/output/...`). Reserved
  namespace; deferred until SDK affordances S1/S2 land.
- **Fan-out / piping** in eventual shell. Workbench-go has open
  proposals; we'd consume their resolution.
- **Capability boundary between apps** when multi-app coexistence
  goes beyond namespace separation.

---

## 10. Coordination items (back to other teams)

Updated after the Go workbench team's alignment review.

**To arch team (next cross-impl pass):**

1. **Pin guide §5.4 Selection schema** as `{path, type, peer_id,
   updated_at}`. Drop `paths[]` and `source_window`. Both impls
   (this repo + workbench-go) have converged on this; arch should
   make it normative before Godot or other impls drift the other
   way. Discussed §5.2.
2. **Document the action-event vocabulary extension process.**
   Guide §6.1 says the list "is not closed"; that invites silent
   divergence (`toggle_expand` vs `expand_toggle` vs
   `toggle_group`). Ask: a documented process for adding canonical
   events — registry + propagation default + cross-impl
   announcement. Discussed §4.4.
3. **Confirm `expanded_paths` as per-impl view-state** stays out
   of the cross-impl schema. We persist it bundled in window-state
   CBOR; Go's tree_model doesn't persist at all today; Godot may
   choose differently. All three are valid per guide §4.3. Just
   noting our choice on record.

**To upstream entity-core-rust team:**

4. **Subscription seed-phase saturation behavior under
   `on_prefix_change_seeded`.** We're about to drive an N-path seed
   through this primitive at Stage A. Confirm the buffer is
   bounded with error propagation (per the §2688 principle and
   our Ask 1+2 landing) rather than silent drop. If our callback
   takes too long under load, we want an explicit signal, not
   missed events.

**To workbench-go team:**

5. **`toggle_expand` naming heads-up.** We're shipping that name
   in our Entity Tree refactor. If you add the same operation
   independently, please match the name (or coordinate via arch
   team if a better name surfaces). See §4.4.
6. **Measurement-feature-flag harness** — you mentioned wanting to
   port our `--features measurement` + per-window render-line
   pattern. Source is in `src/frame_counters.rs` and the
   `dom/mod.rs::update_window_sections` per-window emit; e2e
   capture in `tests/e2e_worker.rs` Phase 11 boundary. Useful
   reference if you build the Go equivalent.

None block Stage A. All deferred to the next coordination pass
once Stage A is in flight or landed.

---

## 11. Resolved before Stage A starts

1. **Window-id → panel-id rename: defer.** Workbench-go renamed
   theirs. Cosmetic in our codebase but touches many files. Not
   blocking Stage A; defer until either (a) we hit the rename
   organically while editing those sites, or (b) cross-impl
   selection-schema persistence forces the rename. Tracked, not
   urgent.
2. **App-id stays `entity-browser`.** Go review confirmed this is
   the right call — namespace is per-instance, coexistence is the
   convention's purpose. Don't rename; we're a distinct app.
3. **Measurement stays in e2e for now.** Per-window render dump
   before the Phase 11 reload is sufficient as a baseline tool;
   move to a `make perf` target only if it earns its keep.
4. **Selection.paths field: drop** (was: optional forward-compat).
   Adopt Go's Stage 5 decision. See §5.2.
5. **`toggle_expand` action: ship under that name.** Mirrors
   guide's `toggle_raw`. Flag to arch team for canonicalization
   process; don't block. See §4.4.

---

## 12. References

- `entity-core-architecture/.../GUIDE-ENTITY-WORKBENCH-APP.md` — cross-impl convention.
- `entity-workbench-go/docs/architecture/SHELL-DIRECTION.md` — shell direction + verb/op pattern.
- `entity-workbench-go/workbench/tree_model.go` — direct reference for Stage A.
- `entity-workbench-go/workbench/ui_tree.go` — tree-node helpers to port.
- `entity-workbench-go/docs/architecture/reviews/FEEDBACK-CROSS-IMPL-UI-PATTERNS.md` — anti-pattern + 5-step migration recipe.
- `PERF-ANALYSIS.md` (this repo) — baseline numbers, refactor priority.
- `UPSTREAM-ASKS-CROSS-IMPL-CONVENTIONS.md` (this repo) — Ask 1 + 2 landed (`len_prefix`, `on_prefix_change_seeded`); these are what we build on.
- `UPSTREAM-ASKS-CROSS-IMPL-REPLY.md` (upstream side) — confirmation of landings.

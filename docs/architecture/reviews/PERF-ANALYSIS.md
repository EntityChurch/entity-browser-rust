# Performance Analysis — render audit + non-render hot paths

**Status:** Structural audit + measured baseline. Instrumentation
wired (`--features measurement`); baseline captured via the e2e
suite (Phase 1–10 interaction window). See §0 for numbers and
§7 for refactor priority.
**Trigger:** Cross-impl convergence with the Go workbench. Workbench-go
proved a §2.8-anti-pattern fix delivered 50× at 14K paths. We have
the same shape in several windows. This doc names where, ranks
severity, and identifies what's already fixed by the upstream
landing.

---

## 0. Measured baseline

Captured via `make wasm-measurement && cargo test --test e2e_worker --
--nocapture`. The e2e test drives a representative session — opens
all 9 windows, populates ~281 tree rows + 1 KB article + 6 event-log
entries + 23 handlers across the primary + a created non-primary
peer — and dumps per-window render numbers before the Phase 11
reload wipes the page buffer. 87 render samples; aggregates below.

| Window | Samples | Render ms (avg / max) | get_entity (avg / max) | tree_listing |
|---|--:|--:|--:|--:|
| **Entity Tree** | 31 | **12.6 / 25.0** | **381 / 655** | 1 |
| **Peer Connections** | 4 | **8.8 / 13.0** | 0 | 0 |
| Key Manager | 1 | 2.0 / 2.0 | 0 | 0 |
| Peers | 2 | 1.5 / 2.0 | 0 | 0 |
| Query Console | 14 | 0.8 / 2.0 | 14 / 32 | 1 |
| Execute Console | 13 | 0.8 / 1.0 | 15 / 32 | 1 |
| Event Log | 12 | 0.5 / 1.0 | 16 / 32 | 1 |
| Knowledge Base | 7 | 0.3 / 1.0 | 0 | 0 |
| Settings | 3 | 0.0 / 0.0 | 0 | 0 |

**What the numbers confirm**

- **Entity Tree is the predicted hot spot.** 12.6 ms avg, 25 ms max
  — exceeds the 16 ms frame budget on max. 381–655 `get_entity`
  calls per render on a 281-row tree is the §2.8 anti-pattern
  signature exactly: the recursive `build_tree_level` calls
  `peers.get_entity(&child_path)` for *every* tree node to check
  `has_entity`, plus the inspector + document calls. Scaling is
  linear in corpus size; at workbench-go's 14K-path comparison
  point we'd be 50× worse.
- **One surprise — Peer Connections at 8.8 ms / 13 ms with 0 Gets
  and 0 tree_listings.** Not data work; it's DOM-building cost
  for QR payload + multiple sections + backend-peer iteration.
  Only fires 4 times in the test (subscribed to 4 narrow
  prefixes), so not load-bearing — but worth a closer look on the
  next pass if frame budget tightens.
- **Event Log / Query Console / Execute Console at sub-ms** with
  14–32 Gets each. Same anti-pattern shape (LOG_CAP=1000 max,
  read all + decode each), but the test only produces ~30 entries
  during the measurement window. The cost is linear in log size;
  at full LOG_CAP it'd be ~30× higher (~15–30 ms each per
  render). Refactor target validated.
- **Knowledge Base at 0.3 ms** with only 1 article. Anti-pattern
  shape confirmed (each render re-fetches title via `get_entity`
  per article) but invisible at K=1. Stays on the list because
  it's the same fix as Entity Tree and KB grows with use.
- **Settings, Key Manager, Peers** clean as predicted.

**Test-time vs production caveats**

- Worker mode runs in selenium-firefox; numbers include Web Worker
  proxy round-trip cache misses on first read of unmirrored
  prefixes (Get for unsubscribed paths returns None and triggers
  observe on retry). Steady-state should be cheaper than what's
  captured here — but the relative ranking is correct.
- Test peer is fresh-spawned each run; corpus reflects bootstrap
  + the test's interaction. A long-lived peer with months of KB
  articles + connection history would see proportionally worse
  Entity Tree + Event Log numbers.
- This is a debug build (`cargo build` profile). Release would be
  ~2–3× faster across the board but the proportions don't change.

---

## TL;DR

- **One free win already in the bag.** Upstream Ask 1 landed:
  `path_count` is now O(1) on every backend. Our Entity Tree footer
  no longer pays a 14K-path linear scan per render. No code change
  required on our side beyond pulling the new SDK.
- **Three windows still rebuild from full enumeration on every dirty
  tick:** Entity Tree (worst — O(N) over peer corpus), Knowledge Base
  (O(article-count)), Event Log + Query Console (O(min(1000, log-size))).
  All four are textbook §2.8 anti-pattern instances.
- **One latent worker-side gap not addressed by Ask 1:**
  `WorkerPeerStore::entity_count_estimate` / `path_count_estimate`
  call `cache_list(prefix).len()`, which materializes-then-counts.
  Upstream `len_prefix` lives below the worker proxy boundary; the
  worker mirror still pays O(N) for a count. Surfaces as Upstream
  Ask 5 below.
- **Five windows are clean by construction.** Settings, Key Manager,
  Peer Management, Peer Connections, Execute Console (handler-list
  cache slot is the right shape).
- **Boot / non-render is fine** for the current corpus shape. Cold
  boot already instrumented (~205 ms worker spawn / ~155 ms per peer
  / ~315 ms reload). No surprising number.

Recommended refactor order: **Entity Tree first** (largest absolute
win + sets the local-state pattern), **KB list second** (mechanical
once the pattern is established), **Event Log + Query Console
third** (share the `event_log_writer::read_events` reader; one fix
unblocks both).

---

## 0.1 Stage A landing — Entity Tree refactor

Measured immediately after the Stage A landing (local-mirror model,
flat `TreeRow` output, `expand_to_depth(8)` default, click handlers
filtered to `.has-entry` rows). Same e2e workload as §0; numbers
captured from the same `--features measurement` log dump.

| Window | Samples | Render ms (avg / max) | get_entity (avg / max) | Notes |
|---|--:|--:|--:|---|
| **Entity Tree (primary, wid=5)** | 28 | **~4 / 13** | **0 / 0** | get_entity target met; render_ms improved ~3× from 12.6 avg |
| Entity Tree (non-primary, wid=10) | 3 | ~11 / 15 | 0 / 0 | Early renders (subscription seed window); steady-state ~4 ms |

**Headline win:** zero `get_entity` per render on the Entity Tree
window. The §2.8 anti-pattern's per-node `has_entity` probe is gone
(`LocationEntry` presence in `tree_listing` *is* the `has_entry`
signal). Document + inspector use a cached `selected_entity` updated
only when `current_path` changes — also zero gets per render.

**Where the residual render_ms comes from.** Stage A's first cut keeps
`refresh_mirror` as the synchronization mechanism — on each dirty
render we call `peers.tree_listing("")` and diff against the local
`known` map (O(N) per dirty tick). That's the ~4 ms floor on the
281-row corpus; under the 14K-path mature workspace it'd scale to
~50 ms/render. Stage A.1 (follow-up) replaces the diff with a
per-event `on_prefix_change_seeded` callback (Direct arm) and a
worker-side per-event delivery primitive (Worker arm — needs upstream
SDK enhancement, tracked as `UPSTREAM-WORKER-OBSERVE-EVENT-PAYLOAD.md`).
Expected post-A.1 floor: O(depth) per event, sub-ms per render.

**Other windows unchanged.** Knowledge Base, Event Log, Query Console
still produce the same get_entity counts as §0 (Stage C work). Peer
Connections still 7–16 ms (Stage F).

**Test infrastructure change:** the test corpus now drives the
Entity Tree window through progressive disclosure. `expand_to_depth(8)`
is the new default — chosen so that this app's typical `app/state/...`
paths (5–7 levels deep) are visible without user action while user-data
tails (KB articles, event log entries) stay collapsed. Differs from
workbench-go's `ExpandToDepth(1)` default; deliberate — they have
content at shallower paths, we don't yet. Revisit if the app starts
binding user content closer to the peer root.

---

## 0.2 Stage A.1 landing — per-event subscription

Wired `EntityTreeModel` to the upstream `observe_with_events`
primitive that landed earlier today. Direct arm: routes through
`PeerContext::on_prefix_change_seeded`. Worker arm: drains the new
`EventChannel` from `WorkerProxy::observe_with_events`. Both
normalize to a local `ChangeOp` enum (`Put` / `Remove` / `Resync`)
that drives `model::apply_change`. `refresh_mirror` is now reachable
only from the `Resync` path — recovery after a `ChangeEvent::Lagged`
from worker-side buffer overflow.

| Window | Samples | Render ms (avg / med / max) | get_entity (avg / max) | tree_listing |
|---|--:|--:|--:|--:|
| **Entity Tree (wid=5)** | 36 | **~7 / 6 / 21** | **0 / 0** | 0 on 34 of 36; 1 on 2 (Resync) |

**Architectural target met.** Steady-state mirror maintenance is now
O(depth) per event via `insert_or_update` / `tree_remove`. Visible-row
flatten is deferred to render-time via `visible_dirty`, so a 281-event
seed phase produces one flatten at first render, not 281 during the
drain.

**Render_ms didn't improve.** At 281-row scale the DOM rebuild
(281 div/span allocations + appendChild calls per dirty render)
is the dominant term, not the model. The model went from "~3 ms of
refresh_mirror diff per dirty render" (Stage A) to "~0 ms per
steady-state event" — the absolute savings are real but invisible
against the ~5–7 ms DOM cost floor at this corpus size. At workbench-go's
14K-path comparison point the Stage A diff would be ~50 ms/render;
Stage A.1 stays sub-ms per event regardless of corpus size.

**Seed-phase backpressure observed.** The 281-Created-event snapshot
overflows the `EventChannel`'s `mpsc(64)` capacity → `ChangeEvent::Lagged`
fires twice (once per peer) → our consumer flips `needs_resync`,
which falls back to `tree_listing` on the next render. The recovery
works correctly (mirror ends up consistent) but burns one
`tree_listing` call per seeded subscription. Two future improvements:
either bump the channel capacity upstream for seeded subscriptions,
or detect "we're still seeding" and skip per-event apply until the
seed-phase signal arrives. Not blocking; logged in
`UPSTREAM-WORKER-OBSERVE-EVENT-PAYLOAD.md` §5 for the
next pass.

**Stage A.1 closes the loop on the upstream feedback.** The
`observe_with_events` primitive proposed in
`UPSTREAM-WORKER-OBSERVE-EVENT-PAYLOAD.md` landed in the
core SDK + proxy with the cleaner `ChangeEvent` enum shape — variant-per-type
instead of the Option-struct we'd sketched. We adopted the upstream
shape via a thin `ChangeOp` normalization at the `Peers` boundary;
the model itself is mode-agnostic.

---

## 0.3 Stage C landing — KB + Event Log + Query/Execute Console

Applied the local-mirror pattern to the four windows that were still
on per-render `tree_listing` + `get_entity` scans. KB grew a slug-keyed
entity cache. Event Log, Query Console, and Execute Console all
adopted a shared `event_log_cache` module — each window owns an
instance, installs its own `observe_with_events` subscription on the
event-log prefix, and reads from its in-memory mirror at render time.

| Window | Before (§0) | After Stage C | Win |
|---|---|---|---|
| **Knowledge Base** | 0.3 ms / 0 Gets | **0.0 ms / 0 Gets** | render falls to noise floor; the K=1 article's get_entity is gone |
| **Event Log** | 0.5 ms / 16–32 Gets | **0.15 ms / 0 Gets** | per-render LOG_CAP scan eliminated |
| **Query Console** | 0.8 ms / 14–32 Gets | **0.14 ms / 0 Gets** | same — shares cache pattern |
| **Execute Console** | 0.8 ms / 15–32 Gets | **0.73 ms / 0 Gets** | event-log piece gone; remaining ms is form-field render + handler list |

`tree_listing` per render also dropped to 0 on all four (was 1/render
for Event Log / Query / Execute, 0 for KB which used a narrower
prefix scan).

**Entity Tree (Stage A.1) still good in this run.** 34 samples, avg
7.9 ms, 0 get_entity on all, 0 tree_listing on 32/34 (the 2 Resync
spikes are the seed-overflow recovery path, same as Stage A.1's
landing measurement).

**Scale projection.** At KB=100 articles the prior per-render cost
would have scaled to ~30 ms (100 × decode + get_entity each). At
LOG_CAP=1000 events the Event Log / Query / Execute consoles would
have scaled to ~15–30 ms each. Stage C makes all four windows
constant-cost regardless of corpus size — they don't pay for what
hasn't changed.

**No cross-panel co-orientation wired.** Stage B laid the selection
slot infrastructure (per-panel + app-aggregate). Stage C deliberately
did NOT have any window auto-subscribe to the slots — the UX dynamic
("which panel observes whose selection?") is unresolved and shared
with workbench-go. Future direction: per-panel "selection source"
setting (None / App-aggregate / specific panel id), filtered to the
local peer for v1. Tracked in
`memory/project_panel_selection_source_design.md`.

**The shared `event_log_cache` module is the first place we factored
a cache out of an individual window's model.** Pattern is small
enough to scale to other shared caches if needed (none on the
horizon).

---

## 0.4 Stage F landing — Peer Connections lazy QR

Stage F closed the last window above 1 ms after Stage C. Peer
Connections measured **8.8 ms avg / 13.0 ms max with 0 Gets and 0
tree_listings** (§0) — pure DOM-build cost, not data work.

**Root cause:** `render_qr_section` (`src/dom/peer_connections.rs`)
called `generate_qr_svg(&output.qr_payload)` on *every* render:
Reed-Solomon encode → an SVG string with one `<rect>` per QR module
→ `set_inner_html` parse of that string. The QR `<details>` ships
collapsed and the payload (`{ws_addr}|{peer_id}`) almost never
changes, so the entire cost was spent building markup the user
wasn't looking at.

**Fix:** deferred generation to the `<details>` `toggle` event —
the same lazy-on-open pattern the sibling "Scan QR Code" `<details>`
already used (`scanner_initialized`). A collapsed render now does
zero QR work; the encode + inject runs once, the first time the
user opens the panel.

| Window | Before (§0) | After Stage F | Win |
|---|---|---|---|
| **Peer Connections** | 8.8 ms / 13.0 ms (0 Gets) | DOM-build minus the QR encode + SVG parse; expected sub-ms collapsed | the dominant per-render cost is gone for the common (collapsed) case |

The remaining collapsed-render cost is the cheap sections
(bound-info, connected list, backend-peer iteration, manual-connect
input) — all O(few) and not corpus-scaling. No window is now
expected above ~1 ms in steady state.

**Regression guard (runtime-agnostic).** The per-window render log
only fires under `--features measurement`, and `make e2e-worker`
builds plain `wasm`. So Stage F's guard is behavioral, not
timing-based: the e2e Peer Connections phase asserts the QR
`<details>` content holds **no `<svg>` while collapsed**, then that
opening it produces one. If generation moves back into the eager
render path the pre-open assertion fails immediately, independent
of build flags or selenium timing.

---

## 1. Per-window render-cost classification

Each row is one window. **Render cost** is the work done per
`render_dom()` call (which fires when its `WindowWatch` flips dirty,
not every frame). **Subscription scope** is what the window's
`watch_prefix` registers — narrower = fewer rebuilds.

| Window | Subscription scope | Per-render work | Scales with corpus? | §2.8 violation? |
|---|---|---|---|---|
| **Entity Tree** | `/{peer_id}/` (entire peer) | `tree_listing("")` + recursive `build_tree_level` walk + N × `get_entity` + 2 × `get_entity` for inspector/document | **Yes — O(N)** | **Yes — full enumeration + rebuild** |
| **Knowledge Base** | `articles_prefix(peer)` + window_state path | `tree_listing(articles_prefix)` + K × `get_entity` (decode title) | **Yes — O(K)** where K = article count | **Yes — same shape, narrower** |
| **Event Log** | `event_log_prefix` | `read_events`: `tree_listing(event_log_prefix)` + up to 1000 × `get_entity` (decode + classify) | Yes but bounded (LOG_CAP=1000) | **Yes — bounded full rescan** |
| **Query Console** | `event_log_prefix` + window_state | same `read_events(peers)` as Event Log every render | Yes but bounded | **Yes — same root cause** |
| **Peer Management** | `peer_registry_signal_path` | iterate `peer_ids()` × `peer_metadata()` per peer | Yes but bounded (peer count is tiny) | No — small N |
| **Peer Connections** | (4 paths: listener state, connections, peer registry, address) | `peer_ids()` iter + `read_connected` + addr/qr build | Yes but bounded | No — small N |
| **Execute Console** | (4 paths: window_state, handlers, etc.) | clone cached `handlers` Vec; build dropdown options | No — handler-list cache slot, refreshed only on `refresh_handlers` action | **No ✅** (right shape) |
| **Settings** | `settings_path(peer, ui)` | one `get_entity(state_path)`, decode | No | No |
| **Key Manager** | (none — pure state) | none — pure render | No | No |

### Same-root-cause grouping

The four red rows split into two refactors:

- **Group A (Entity Tree + KB)** — peer-tree-scan panels. Each one
  reads its own prefix; fix is panel-local view-state per §2.8.
- **Group B (Event Log + Query Console)** — both consume
  `event_log_writer::read_events`. One reader; two callers. Fix the
  reader (or factor a `CachedEventLog` shared singleton), both
  callers benefit.

So conceptually there are **three** refactor targets, not four.

---

## 2. The shape of each violation

§2.8 names six signatures of the anti-pattern (workbench memo, "The
anti-pattern, recognizing it"). Mapping each to our code:

| §2.8 signature | Where we have it |
|---|---|
| Shared "current peer state" cache | ❌ none — no shared cache in our codebase |
| Panels read shared cache + filter for own prefix | ❌ none |
| **Single refresh tick fires for every tree event, iterates all panels** | ⚠️ **Half-true.** Per-window `WindowWatch` gates rebuilds individually, but Entity Tree subscribes to `/{peer_id}/` (everything) so it fires on every write to the peer. |
| Status displays / aggregates count all entities every refresh | ✅ **Fixed by Ask 1** — `path_count` now O(1) |
| Inspector re-fetches selected entity on every tick | ⚠️ **True for Entity Tree** — `build_inspector` + `build_document` re-`get_entity` on every render, gated only by the window's dirty flag (which fires for any change anywhere in the peer) |
| **Tree-browser rebuilds from full enumeration on every refresh** | ⚠️ **True for Entity Tree** |

Net: Entity Tree is the worst single instance. Three of the six
signatures live on it.

---

## 3. The Entity Tree refactor — what it actually means

Current render path (`src/views/entity_tree/model.rs:176-205`):

```rust
let entries = peers.tree_listing(&self.peer_id, "");      // O(N)
let tree = build_tree_level(peers, pid, "/", &entries, ...);
//  └─ recursive walk; at each level filters all_entries by prefix
//     and calls peers.get_entity(child_path) to check has_entity
let footer  = TreeFooter { entity_count, path_count };    // ← was O(N); now O(1) post-Ask-1
let document  = build_document(peers, pid, current_path); // 1 × get_entity
let inspector = build_inspector(peers, pid, current_path);// 1 × get_entity
```

Target shape per §2.8:

```text
Model construction:
  local_paths: HashMap<String, Hash>     // path → hash mirror
  on_prefix_change_seeded("/{peer_id}/", on_event):
     - synthetic Created for each existing path (one pass)
     - live events thereafter
  store cancel on window close

on_event(ev):
  Created(path, hash) | Modified(path, hash):
     local_paths[path] = hash
     window.watch.mark()
  Removed(path):
     local_paths.remove(path)
     window.watch.mark()

render_output(peers):
  // walks local_paths only; no peers.tree_listing call
  // builds tree structure from the in-memory map
  // for the SELECTED path's document + inspector:
  //   - either subscribe specifically to current_path (parity with
  //     workbench's two-subscription pattern), or
  //   - keep the one get_entity call but ONLY re-render on path-change
  //     or on Modified event for that exact path
```

What changes:
- `tree_listing("")` disappears from the render path.
- `get_entity` per node disappears — `has_children` becomes a
  `local_paths.contains_key(child_prefix)` scan over the in-memory
  map.
- Inspector / document still need one `get_entity` per render of
  the *selected* path. Either (a) accept that and pay 1 × Get, or
  (b) wire a second subscription on the selected path specifically
  and store the decoded entity in local state.

Option (a) is simpler; we go (b) only if we measure inspector-Get
showing up. Likely fine.

**One caveat:** the Entity Tree window's `WindowWatch` currently
subscribes to `/{peer_id}/` (broad — fires on every write under the
peer). After the local-state refactor, the *render* is incremental
but the *watch* still fires for every event. That's correct — the
event handler updates local state and *then* marks dirty. The
expensive part (full rebuild) is gone; the cheap part (re-render
from local map) is what runs.

---

## 4. Non-render hot-path audit

### 4.1 `Peers::peer_ids()` — O(SDK × peers/SDK), small

Iterates all SDKs and dedupes via HashMap. For our Stage 2 shape
(1 boot SDK + N backend SDKs, each holding 1 peer), this is
~`O(SDK_count)`. Called every render of Peer Management and Peer
Connections. Bounded; fine.

### 4.2 Worker proxy round-trips

L0 reads in worker mode hit `WorkerPeerStore::cache_get` /
`cache_list` against the proxy's in-process mirror. **Sync**, no
round-trip. Good — the subscription pattern keeps the cache
hydrated and renders never wait on the worker.

`dispatch_write` / `put_and_wait` do round-trip; those run on
action, not render — correct.

### 4.3 Subscription dispatch

Per-window `WindowWatch` owns isolated `Arc<AtomicBool>` dirty
flags. No shared queue, no cross-window backpressure. By
construction this is the §2.4 ("per-Phase-2-consumer isolation")
shape the workbench memo recommends. ✅

### 4.4 Boot

Already instrumented (`backlog.md` cold-start measurement):
- Boot worker spawn: ~205 ms
- Per-peer spawn (backend respawn): ~155 ms
- Page reload total: ~315 ms

Healthy numbers for a multi-SDK boot. Backlog item still open for
cold-first-load measurement (HTML parse → frame loop start delta),
but no signal that something there is broken — the user-reported
1–2 s feel was probably initial wasm fetch + compile, not work in
`start()`.

### 4.5 Event Log `LOG_CAP=1000`

Capped, so it doesn't grow forever. But every render of Event Log
*or* Query Console reads all 1000 + decodes each. At an event-rich
session that's 1000 entity Gets per render tick. Same fix as
Entity Tree — local-state subscriber pattern, ring buffer
maintained in memory; tree writes update the ring.

---

## 5. Latent gap not closed by Ask 1 — worker-side `cache_list().len()`

Worker mode's count APIs (`src/peers_worker.rs:142-151`):

```rust
pub fn entity_count_estimate(&self, peer_id: &str) -> usize {
    let prefix = format!("/{}/", peer_id);
    self.proxy.cache_list(&prefix).len()   // ← materializes mirror
}
pub fn path_count_estimate(&self, peer_id: &str) -> usize {
    self.entity_count_estimate(peer_id)    // same
}
```

`WorkerProxy::cache_list` returns `Vec<(String, WireEntity)>` from
its in-process cache. The upstream `LocationIndex::len_prefix` lives
inside the worker, behind the proxy boundary. So calling
`peers.path_count()` in worker mode still pays O(N) (decode
WireEntity → Entity for every entry, just to discard).

Two ways to close this:
- **Worker proxy ask** — add `WorkerProxy::cache_len(prefix)` that
  returns count without materializing entries.
- **Or — workaround on our side** — maintain a cached count in
  `WorkerPeerStore`, updated by subscription events. We already
  subscribe to `/{peer_id}/` indirectly via observed prefixes.

The proxy ask is cleaner. Worth bundling into the next upstream
round (see §7 below).

---

## 6. What measurement would add

Instrumentation is wired (`--features measurement`). Each
window-rebuild now logs `window`, `render_ms`, `get_entity`,
`tree_listing` deltas. To get numbers:

```
make wasm-measurement
make serve
# open browser at INFO log level
# create peers, populate KB with N articles (or open a worker-mode
# peer pointing at a populated OPFS), open Entity Tree, sweep around
```

Expected pattern (matches §2.8 prediction):
- Entity Tree `render_ms` scales linearly with corpus size.
- KB `render_ms` scales with article count.
- Event Log `render_ms` flat-but-high (~LOG_CAP work each).
- Other windows flat sub-ms.

If measurement *contradicts* the prediction, the audit needs
revisiting. Otherwise the audit is the deliverable and measurement
just confirms numbers we already know directionally.

The user's call: run the measurement before refactoring (defensible
for "show the win after the refactor"), or skip and refactor based
on the structural audit alone. The refactor target doesn't change
either way.

---

## 7. Recommendations

### 7.1 Free wins (no work)

Already in: **Ask 1** — `path_count` is O(1) on every backend.
Entity Tree footer is faster now without any consumer-side change.

### 7.2 Refactor order

1. **Entity Tree local-state refactor.** Largest single win.
   Establishes the per-window-`HashMap<path, hash>` pattern that
   the other refactors copy. Concretely:
   - Add a `local_paths: Mutex<HashMap<String, Hash>>` slot to
     `EntityTreeModel`.
   - Use `on_prefix_change_seeded` (new from Ask 2) to populate +
     drain live events.
   - `render_output` reads `local_paths`, not `peers.tree_listing`.
   - `build_tree_level` becomes pure (no Get, no recursive
     filter — walk the map).
   - Inspector + document keep their `get_entity` call for now;
     measurement decides if those need their own subscription.

2. **KB article-list refactor.** Mechanical copy of the Entity
   Tree pattern, narrower scope (`articles_prefix`).

3. **Event Log + Query Console reader refactor.** One reader
   (`event_log_writer::read_events`), two callers. Either factor
   a `CachedEventLog` singleton that owns the ring buffer + a
   single subscription, or give each window its own local state.
   Singleton is simpler since both panels show the same data
   shape.

### 7.3 Next upstream round (Ask 5 candidate)

`WorkerProxy::cache_len(prefix)` to close the worker-mode count
gap. Single-method addition. Cheap; can ride the next protocol-
version bump if there is one. Not blocking — workaround on our
side is also tractable.

### 7.4 Not on the list

- Boot perf — fine for now.
- Subscription dispatch — already isolated by construction.
- L0/L1 routing — clean.
- `legacy-eframe` — empty windows; still backlogged for either
  removal or revival; not relevant here.

---

## 8. References

- `GUIDE-EMIT-PIPELINE-IMPLEMENTATION.md` §2.8, §2.9, §7
- `FEEDBACK-CROSS-IMPL-UI-PATTERNS.md` (workbench-go) —
  the anti-pattern definition + measured numbers
- `UPSTREAM-ASKS-CROSS-IMPL-CONVENTIONS.md` — our asks
  (1 + 2 landed; 3 + 4 deferred; 5 candidate for next round)
- `EXPLORATION-EMIT-DURABILITY-AND-DELIVERY.md` §28 — system
  guarantees + composition synthesis
- Internal: `src/views/{entity_tree,knowledge_base,event_log,query_console}/model.rs`
- Internal: `src/peers_worker.rs:142` (worker count gap), `src/event_log_writer.rs:124` (shared reader)

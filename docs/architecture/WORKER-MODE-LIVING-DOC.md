# Worker Mode — Consumer-Side Living Document

**Status:** Living. Update in-place as understanding evolves.
**Owns:** Patterns, issues, decisions, and status from the consumer
(entity-browser-rust) perspective. The kernel/proxy/host design is
owned upstream in entity-core-rust.

---

## 1. Current architecture

`Peers` holds a `Vec<Sdk>` (`Sdk = Direct | Worker`); the arm is
**per-SDK / per-peer**, and **mixed mode (a Direct primary plus
Worker backend peers) is normal**. The per-arm mechanics below
describe how each `Sdk` variant behaves. For the canonical model of
the `Peers`/`Sdk` relationship, the full combinatorial surface, the
arm-split footgun, and the lifecycle-API refactor, see
`reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md`.

Each `Sdk` in `Peers::sdks` is one of two arms:

- **Direct (`Sdk::Direct`)** — a `PeerManager` on the main thread.
  Sync L0 store access, sync L1 dispatch (under tokio). The boot SDK
  is Direct for native builds, `make wasm` (no worker), Tauri
  Linux, tests, and as the **fallback** when worker bootstrap fails.
- **Worker (`Sdk::Worker`)** — a `WorkerPeerStore` over a dedicated
  Web Worker (`WorkerProxy` / postMessage). The boot SDK is Worker
  for browser worker boot; **every `BackendMemory`/`BackendOpfs`
  peer additionally gets its own dedicated Worker SDK**, regardless
  of the boot arm.

Router (`peer_id`-taking) methods present the same `Peers` API on
both arms — view code must not branch on arm. **Lifecycle ops
(`delete_peer`/`create_new_peer`/…) do leak the arm** via twin
APIs / primary-only methods; that is the defect tracked in
`reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` — controllers must
decide the arm from the *target peer's* SDK, never the primary's.

### Worker pipeline (main → worker → main)

1. Main thread action handler calls `peers.dispatch_write(path, entity)`.
2. `WorkerProxy::put` serializes a wire request and `postMessage`s the
   worker.
3. Worker dispatches through SDK L1 (`system/tree:put`), committing
   the write to the worker's tree.
4. L1 subscription engine fires callbacks for matching subs.
5. The host's L1 callback reads the entity via `ContentLookup::get_by_hash`
   (sync L0 from inside the worker) and posts `Event::Change` with
   the entity inline.
6. Main-thread proxy's demultiplexer receives `Event::Change`, applies
   it to the per-subscription mirror, fires the notify channel.
7. Our `WindowWatch` bridging task sees notify, flips the dirty flag.
8. Next render frame: dirty windows rebuild their DOM. Their model's
   `render_output(peers)` reads from the mirror via `cache_get` /
   `cache_list`.

Latency end-to-end (uncontended): ~5–20ms. The pipeline is async, but
under normal conditions each step completes within a frame budget.

---

## 2. Patterns

These are the codified conventions for new views. Deviations create
the kind of bugs documented in §3.

### 2.1 Domain content lives in the tree

Entities the user creates or views — articles, query results, tree
nodes, log entries — **read fresh from the peer tree at render time**.
Do not maintain a parallel cache in the window model. The tree
(proxy mirror in worker mode, in-memory store in direct mode) is the
single source of truth.

```rust
// Right:
pub fn render_output(&self, peers: &Peers) -> Output {
    let articles = peers.tree_listing(&self.peer_id, articles_prefix());
    let current = current_slug.and_then(|s| peers.get_entity(&self.peer_id, &article_path(s)));
    // build Output from these reads
}

// Wrong (parallel cache):
struct ModelInner {
    state: WindowState,
    articles: Vec<Article>,     // ← shadows tree
    current_article: Option<...>,// ← shadows tree
}
```

The parallel-cache antipattern bit Knowledge Base: optimistic
window-model state showed an article that `cache_get` then returned
`None` for on click, surfacing "selected article is no longer
available." Single source of truth eliminates that divergence class.

### 2.2 Window UI state lives in the tree (persisted)

State that should survive page reload — view mode, current selection,
form values like host:port, expanded panels — goes in a window-state
entity at `/{peer_id}/app/entity-browser/workspace/windows/{id}/state`.

Models hold an in-memory copy of this state in `Arc<Mutex<Inner>>`,
mutated by action handlers and persisted via `dispatch_write`. On
window-factory creation, `initialize(peers)` hydrates the in-memory
state from the tree. Every view today follows this pattern except
the parts of KB that we just refactored.

### 2.3 Transient UI state is DOM-only

State that shouldn't persist across refresh — text being typed in a
form, scroll position, drag state, hover effects — lives in the DOM
input/textarea values and is read at the moment it's used. The KB
save flow demonstrates this: the title and content fields are read
out of the textarea at the save button's click handler.

If a future feature wants draft persistence (don't lose what you've
typed when navigating away), that's a separate concern: a transient
"draft" entity, intentionally distinct from the article entity.
Don't conflate it with the cache layer.

### 2.4 Subscriptions are for *external* state changes

A subscription is a reaction to state the consumer didn't initiate.
Self-initiated writes (an action handler that mutates window state and
writes to the tree) should *also* trigger a re-render, but the
mechanism is the same subscription pipeline — the write fires the L1
callback, the callback flows back to the proxy, the notify channel
fires, dirty flips.

If you find yourself reaching for `self.watch.mark_dirty()` directly
in an action handler, that's a smell. It usually means the
subscription pipeline isn't doing its job — most often because of
the L1 fan-out bug (§3.1). Fix the pipeline, don't mark dirty.

(Both Entity Tree and KB previously held `mark_dirty()` calls in
their `handle_action` as masks while the L1 fan-out bug in §3.1 was
open. Both were removed once the upstream fix landed and was verified
end-to-end. Don't re-introduce them; if a similar symptom shows up,
the subscription pipeline has regressed and that's the real bug.)

### 2.5 Read-your-own-write needs explicit synchronization

Action handlers that write and then transition the view to display
what was written must wait for the write to land before the
transition. Use `proxy.put_and_wait_for_cache(...)` (worker mode) or
its equivalent. `save_article` should be async, await cache
confirmation, then transition `view_mode` to Reader.

Dropping the optimistic update without also wiring the await opens a
brief but visible race window. The current KB save flow awaits cache
confirmation (see §3.2).

---

## 3. Open issues

Severity legend: **H** (blocks correct behavior in a real scenario),
**M** (real bug, has a UX impact, working around), **L** (architectural
clean-up, no functional impact).

### 3.1 L1 subscription fan-out — ✅ FIXED

**Original symptom:** A write to `/peer/foo/bar` notified the
`/peer/foo/bar` subscription but not the broader `/peer/foo/` or
`/peer/` subscriptions.

**Root cause** (refined from initial diagnosis): The kernel's L1
subscription engine fanned out correctly to all matching *patterns*.
The bug was at the wire→SDK boundary in `wasm-worker-host`: wire
prefixes like `/peer/` were passed straight through to the SDK as
literal-match patterns, when SDK patterns require `/*` suffix for
prefix matching. The host did not translate.

**Fix:** Added `prefix_to_pattern` translator in `handle_subscribe`
that appends `/*` (idempotent for inputs already ending in `*`).
After this, a wire prefix like `/peer/` correctly fans out to every
descendant write.

**Verification:** Entity Tree's `mark_dirty()` mask in
`src/views/entity_tree/mod.rs::handle_action` was removed. E2E
(`tests/e2e_worker.rs` Phase 5) passes the `inspector_populated`
assertion — proving the subscription pipeline alone drives the
re-render after a self-initiated Navigate. The `tree_items` count
grows once post-subscribe Change events actually populate the broad
mirror.

### 3.2 Read-your-own-write timing — ✅ FIXED

**Original symptom:** After `dispatch_write`, the just-written
entity wasn't in the cache mirror until the Change event landed
(~10–20ms). Action handlers that transitioned the view to display
the new entity saw an empty cache for a render frame, briefly
showing "no longer available" before the content appeared.

**Fix:** `Peers::put_and_wait` wraps
`proxy.put_and_wait_for_cache` (worker arm) and `ctx.put` (Direct
arm). KB's "save" branch in `handle_action` now:

1. Calls `model.prepare_save(title, content)` — sync validation +
   entity encoding, no state mutation.
2. Constructs `article_future = peers.put_and_wait(...)` from the
   peers borrow.
3. Calls `model.commit_view_after_save(target_slug)` — switches
   in-memory view_mode to Reader.
4. Constructs `state_future = peers.put_and_wait(state_path,
   model.window_state_entity())` for the post-save window state.
5. Spawns: `article_future.await; state_future.await`.

Ordering: the article put_and_wait completes (cache reflects)
BEFORE the state put is issued. So whichever subscription's notify
flips dirty first (articles_prefix or window_state), the article
is already cache-resident at render time. No flash.

**Verified:** E2E Phase 6 still passes (KB save→back→click round
trip green). Native: 124/124 tests pass via the legacy
`save_article` test-helper which now delegates to `prepare_save`
+ `commit_view_after_save`.

**References:** `src/peers.rs` (Peers::put_and_wait),
`src/peers_worker.rs` (WorkerPeerStore::put_and_wait),
`src/views/knowledge_base/mod.rs` "save" branch,
`src/views/knowledge_base/model.rs::prepare_save` /
`commit_view_after_save` / `window_state_entity`.

### 3.3 Worker-mode networking primitives — M — design

**Symptom:** `handle_start_listener` and `handle_connect_peer` need
native sockets. In worker mode the call path panics on
`peer_shared()` first (covered in §3.5), but even with that
plumbed, browser workers cannot directly accept incoming sockets
or initiate TCP — that's a JS/runtime capability, not in the
SDK's surface.

**Constraints:**
- **Browser-only WASM:** workers can't `bind` a TCP/WS listener
  (no `Deno.listen` / `net.Server` equivalent). Outgoing
  WebSocket clients work via the `WebSocket` API, but listening
  doesn't. Plain-WASM worker deployments are client-only.
- **Tauri:** the WebView is a browser context with the same
  constraint, but the Tauri Rust side CAN listen. So Tauri-side
  listener via IPC works (already partly wired via backend-peer
  Tauri actions).

**Design directions:**
- **Worker mode is client-only.** UI gates the Listener buttons
  off when running in Worker mode (introspect `Peers` arm at
  render). Connect is fine — outgoing `WebSocket` works from a
  worker, just needs a worker-arm proxy method
  (`proxy.connect_peer`).
- **Tauri-side listening.** Separate concern from Worker
  parity; goes through Tauri IPC, not the worker boundary.
  Already designed for backend-peer creation; needs extension
  to listener.

**Effort:** ~200 LOC for worker-arm `connect_peer` + UI gating.
Listener for Tauri is its own work item.

**References:** §3.5 for the call-site panics; backend-peer
flow in `app.rs::handle_create_backend_peer`.

### 3.4 `cache_get` semantics for paths outside any subscription — L — design

**Symptom:** `proxy.cache_get(path)` walks every subscription's mirror
and returns the first match. If no subscription's prefix covers the
path, returns `None` even if the path exists in the worker's tree.

**Status:** Expected behavior for a sub-driven mirror. Worth
documenting that consumers calling `cache_get` for paths outside their
own subscription scope can get stale-or-missing results that don't
reflect actual tree state.

### 3.5 Worker-mode API parity gaps — H — wiring work

The `Peers` enum has 12 methods that panic on the Worker arm. Some
are intentional escape hatches (Direct-mode-only internals).
Others are real functional gaps the user hits via UI actions.

**Functional gaps (worker users hit these, app panics):**

| Method | Hit by | Action / flow | Status |
|---|---|---|---|
| `create_new_peer` | `app.rs:401` | `Action::CreatePeer` — add peer in UI | ✅ Parity-B |
| `delete_peer` | `app.rs:453` | `Action::DeletePeer` — remove peer in UI | ✅ Parity-B |
| `sdk_mut().set_metadata` | `app.rs:406, 898` | rename peer / set label after create | ✅ Parity-C (wire-ready; no current UI call site) |
| `peer_context` for new pid | `app.rs:415` | spawn `event_bridge` after peer create | Parity-B follow-up |
| `peer_context` in handle_query | `app.rs:647` | `Action::Query` — Query Console submit | ✅ Parity-A |
| `peer_context` in handle_count | `app.rs:694` | `Action::Count` — Query Console count | ✅ Parity-A |
| `peer_context` in handle_execute | `app.rs:720` | `Action::Execute` — Execute Console run | ✅ Parity-A |
| `peer_shared` in handle_start_listener | `app.rs:596` | also blocked by §3.3 native-ws | §3.3 |
| `peer_shared` in handle_connect_peer | `app.rs:773` | outgoing remote-peer connect | ✅ Parity-D-narrow. Callers go through uniform `Peers::connect_peer` (no arm choice); only the post-connect type fetch still branches (proxy.execute for Worker, make_execute_fn for Direct). `Sdk::peer_shared` returns None on Worker arm instead of panicking. |
| `discover_handlers` | `execute_console/model.rs:245` | Execute Console handler dropdown | ✅ Parity-A |
| `sdk().peer_ids` / `peer_metadata` | `app.rs:90,314,896` | peer enumeration on bootstrap | Parity-C |
| `peer_registry_signal.rs:45` `peer_shared` | peer-display registry bump | peer display name in palette | Parity-C |
| `event_log_writer.log()` worker stub | All event-log writers | result lines silently dropped in worker mode | ✅ Parity-A follow-up |

**What works without panic in worker mode today:**

| Surface | Mechanism |
|---|---|
| Tree reads (`get_entity`, `tree_listing`) | Proxy cache mirror |
| Counts (`entity_count`, `path_count`) | Proxy `entity_count` / `path_count` arms |
| Writes (`dispatch_write`) | Proxy `put` (fire-and-forget) |
| Removes (`dispatch_remove`) | Proxy `remove` |
| Subscriptions (`watch_prefix`) | Proxy `observe` |
| Window UI: Settings, Entity Tree, Knowledge Base | Verified by E2E |

**Intentional escape hatches (worker shouldn't use, by design):**

| Method | Replacement |
|---|---|
| `peer_context`, `peer_context_or_default` | Use proxy methods directly via `Peers::Worker` arm |
| `peer_shared` | n/a — `PeerShared` is direct-mode internal |
| `sdk`, `sdk_mut` | Use proxy methods; metadata via `peer_metadata` |
| `peer` | n/a — `Peer` is direct-mode internal |
| `put_entity` (sync, returns hash) | Use `dispatch_write` |
| `load_persisted` | Worker arm loads via `InitParams` |
| `register_backend_peer` | Use `Request::RegisterBackendPeer` |

**Trajectory to parity** (proposed phasing):

- **Parity-A** (unblocks Query/Execute Console migration): refactor
  `handle_query` / `handle_count` / `handle_execute` to NOT call
  `peer_context` in worker mode. Instead, route through new
  `Peers::execute / query / count` methods that branch:
  - Direct: `ctx.execute/query/count.await`.
  - Worker: `proxy.execute/query/count.await`.
  Same for `discover_handlers`. Estimated ~150 LOC.
- ✅ **Parity-B** (peer creation/deletion in worker mode) — landed
  at PROTOCOL_VERSION=4. Wire: `Request::CreatePeer
  { label }` returning `CreatePeerOk { peer_id, keypair_seed,
  metadata }`; `Request::DeletePeer { peer_id }`. Proxy:
  `create_peer / delete_peer` async methods. Host: keypair gen
  inside the worker (browser getrandom) + sdk.create_peer +
  set_metadata. Consumer-side: `Peers::create_new_peer_worker /
  delete_peer_worker` futures branch into `WorkerPeerStore`,
  which RefCell-mutates the peer mirror on success and persists
  the seed via localStorage. `peer_registry_signal` extended with
  worker-arm `proxy.put` to drive re-render
  (mirrors `event_log_writer` pattern). E2E Phase 12 verifies
  end-to-end. (~300 LOC upstream + ~100 LOC consumer, including
  the signal worker-arm fix.)
- ✅ **Parity-C** (peer metadata sync) — landed at
  PROTOCOL_VERSION=5. `Request::SetMetadata { peer_id, metadata:
  WirePeerMetadata }` returning `Option<WireError>`. Reuses
  `WirePeerMetadata` from v4. Consumer side:
  `WorkerPeerStore::set_metadata` updates the main-thread peer
  mirror on success; `Peers::set_metadata_worker` boxed-future
  branching wrapper. No current UI surface calls this — the
  Parity-B `CreatePeerOk` flow already returns the metadata
  inline from the worker — so the Worker-arm method carries
  `#[allow(dead_code)]` and exists for future rename / label-edit
  flows.
- ✅ **Parity-D-narrow** (outgoing ConnectPeer) — wire landed
  at PROTOCOL_VERSION=5. `Request::ConnectPeer {
  peer_id, address }` returning `Result<ConnectPeerOk { remote_peer_id },
  WireError>`. Host inlines the four-step `Peer::connect_to` body
  (connector.connect → perform_connect → remote.insert) to avoid
  holding a `&Peer` across the network await. Consumer side:
  `app.rs::handle_connect_peer` branches — Direct keeps the existing
  inline-connect flow; Worker spawns `Peers::connect_peer_worker`
  and follows up with a Parity-A `proxy.execute(entity://{remote}/system/tree)`
  type fetch for the same UX as Direct. Pooling stays inside the
  worker; subsequent execute round-trips against the remote URI
  flow through Parity-A's existing arm and find the pooled connection
  there. ~80 LOC consumer-side.
  ✅ **Connector bug fixed.** An early end-to-end attempt surfaced
  that the worker's primary peer was built without `.connector(...)`,
  so `Peer::connect_to(addr)` failed with "no connector configured"
  before any network I/O. Upstream added the missing two lines in
  `bindings/wasm-worker-host/src/lib.rs` (the additional-peers path
  already had the connector; only the primary builder was missing
  it). No wire change.
- **Parity-D-listener**: cannot be wired. Browser workers cannot
  bind sockets. UI gates Listener buttons off in worker mode.
  Tauri-side listener via the src-tauri/ native Rust backend +
  IPC is a separate path, unchanged.

Phase ordering: A unblocked two already-prepared windows, so it
shipped first. Parity-B followed because it was the only UI
panic-on-click in the worker matrix at that point; C and D-narrow
shipped together in the final closeout, both small and sharing the
established wire-protocol pattern. Phase 4 (bounded-LRU eviction) is
deferred as scale-triggered.

### 3.6 Tauri Linux WebKitGTK worker OPFS gap — M — runtime-specific

**Discovered** during the post-Parity-B close-out, when
`make tauri-run` switched to `wasm-worker-release` and the WebView
crashed during worker spawn:

```
worker spawn: InitFailed("SDK build failed: peer build failed:
build error: opfs: OPFS unavailable: no storage.getDirectory:
JsValue(TypeError: Reflect.get requires the first argument be an
object)")
```

**Root cause:** WebKitGTK's `WorkerNavigator` does not expose
`storage` (the `StorageManager` property) in worker context, even
on WebKitGTK 2.52 (Sept 2024). The same engine on Apple platforms
shipped this in Safari 15.2 (Dec 2021) per
[caniuse](https://caniuse.com/mdn-api_storagemanager_getdirectory) /
[MDN](https://developer.mozilla.org/en-US/docs/Web/API/WorkerNavigator/storage).
WebKitGTK is a separate codebase variant maintained by
Igalia/GNOME that lags Apple's upstream WebKit; this feature is one
of the trailing ones.

**Scope of breakage:**

| Deployment | Status |
|---|---|
| Browser: Firefox / Chrome / Edge | ✅ works (Chrome 86+, Firefox 111+) |
| Browser: Safari iOS 15.2+ | ✅ works |
| Browser: Safari macOS 15.2+ | ✅ works |
| Tauri Linux (WebKitGTK ≤ 2.52) | ❌ broken |
| Tauri macOS (WKWebView, tracks Safari) | Untested. Likely works on macOS 12+. |
| Tauri Windows (WebView2 / Blink) | Untested. Should work. |

So in practice this is a **Tauri Linux-only gap**. The whole
browser-deployment matrix is fine.

**Why we missed it in the close-out review:** Our E2E
(`tests/e2e_worker.rs`) drives Firefox via Selenium. Firefox
supports worker OPFS, so the test stack was green. We did not
test inside Tauri's actual WebView (WebKitGTK) before declaring
done. Process lesson: when shipping persistence-sensitive code
across deployment targets, exercise each runtime, not just one.

**Workarounds (in order of effort):**

1. **Status quo for Tauri** (currently): Makefile points
   `tauri:` at `wasm-release` (Direct mode, no OPFS). Tauri
   Linux runs without persistence — keypairs survive via
   localStorage, tree state is in-memory and lost on close.
   Same state Tauri was in before this session began.

2. **Direct mode + main-thread OPFS in Tauri**: Add an async
   `EntityApp::new_wasm_with_opfs()` constructor that calls
   `EntitySDK::builder().opfs().build_async().await`. The
   primitive is already on the SDK (landed Phase 2). Browser
   stays on worker mode; Tauri switches to Direct+OPFS. Two
   configurations, each picks the right path. ~30 LOC,
   consumer-side only.

3. **Worker mode + main-thread OPFS handle transfer**:
   Universal fix. Main thread opens
   `Navigator.storage.getDirectory()` (works in every WebKit),
   transfers the `FileSystemDirectoryHandle` to the worker via
   postMessage transfer list, worker uses pre-opened handle.
   Requires upstream: `OpfsStore::open_with_handle(handle)` +
   protocol extension to ship a transferable in `InitParams`.
   ~80 LOC upstream + ~30 LOC consumer.

4. **Wait for WebKitGTK to land it upstream.** No timeline
   visibility. WebKitGTK's WebKit roll-forward isn't tracked
   in any public-facing roadmap we know of.

**Current call:** Option 1 (status quo for Tauri,
worker+OPFS for browser). Browser is the primary deployment;
Tauri Linux is for developer use and tolerates in-memory state.
Revisit if/when Tauri Linux becomes a user-facing target — at
that point Option 2 is the smallest fix; Option 3 only if we
also want Tauri Mac/Windows to share architecture with browser.

**References:** `entity-core-rust/core/store/src/opfs.rs:144-156`
(the probe that fails — three `Reflect::get` calls walking
global → navigator → storage → getDirectory). The trace shows
`navigator.storage` returns undefined, so the next Reflect.get
throws.

### 3.7 App-tier writers — use [`WriterHandle`], no per-arm boilerplate

App-tier modules that publish to the system peer's tree from
clonable handles (`event_log_writer`, `peer_registry_signal`,
`connections`, etc.) hold a [`crate::writer_handle::WriterHandle`]
obtained via `peers.writer_handle()`. The handle wraps either an
`Arc<PeerShared>` (Direct) or `Rc<WorkerProxy>` (Worker) and
exposes a uniform `put(path, entity)` / `remove(path)` surface.
The arm-branching is owned by the handle, not by each writer.

**Reference template (shape after the refactor):**

```rust
#[derive(Clone)]
pub struct MyWriter {
    system_peer_id: String,
    handle: Option<WriterHandle>,
}

impl MyWriter {
    pub fn new(peers: &Peers) -> Self {
        Self {
            system_peer_id: peers.primary_peer_id().to_string(),
            handle: peers.writer_handle(),
        }
    }

    pub fn write_thing(&self, ...) {
        let Some(h) = &self.handle else { return };
        h.put(path, entity);
    }
}
```

That's it. No cfg gates, no dual fields, no per-call branching, no
silent stub-arm failure mode.

**History (why this section exists):** Before the refactor, each
writer carried both an `Option<Arc<PeerShared>>` and an
`Option<Rc<WorkerProxy>>` field plus matching cfg-gated branches in
its write method. The Worker arm was easy to forget, and a
stub-only ("trace stub") Worker arm compiled cleanly while silently
no-op'ing — bit us three times in one session before the
abstraction landed:

1. `event_log_writer::log()` — Parity-A close-out. Result lines
   silently dropped in worker mode.
2. `peer_registry_signal::bump()` — Parity-B. Peers window row
   count didn't grow after CreatePeer.
3. `connections::ConnectionsWriter::add()` — Parity-D-narrow.
   Execute Console peer dropdown didn't show the new peer.

After the third instance the abstraction was extracted instead of
codifying the pattern. Net: ~90 LOC of dual-arm boilerplate
removed, ~80 LOC added in `WriterHandle`, the bug class is now
unrepresentable.

**Audit shortcut:** any `#[derive(Clone)] pub struct *Writer` (or
similar app-tier signaling type) in `src/` should hold an
`Option<WriterHandle>` and not branch on arm. Today:
`event_log_writer`, `peer_registry_signal`, `connections` all
follow this. `listener_state` is Direct-only by design (worker
can't bind sockets per §3.3, gated to `feature = "native-ws"`)
and so doesn't need the abstraction.

### 3.8 Subscribe wire variant lacks peer_id — ✅ FIXED (PROTOCOL_VERSION=6)

**Discovered** when a user opened Entity Tree on a non-primary
local peer; tree displayed correctly but row clicks didn't populate
the inspector. The dispatch_write log line for the window-state path
DID appear, confirming the click registered and saved — but no
re-render happened.

**Root cause:** `Request::Subscribe { request_id, sub_id, prefix }`
is the only Request variant on the wire that doesn't carry
`peer_id`. The protocol comment says "peer_id is in the prefix";
host's `handle_subscribe` (`wasm-worker-host/src/lib.rs:1133`)
hardcodes `sdk.default_peer_id()` and registers the L1 callback on
the primary peer's dispatch only. Writes through any other peer's
L1 fire that peer's callbacks — but the consumer's subscription is
listening on primary's, so no Change event reaches the proxy
mirror. Mirror never updates → notify channel never fires → window
dirty flag never flips → render never runs.

**Why it appears to half-work:** the initial Snapshot is built via
`peer_ctx.store().list(prefix)`, and the underlying `entity-store`
is shared across all peers in the SDK (per-peer namespacing is
path-prefix convention, not physical isolation). So
`primary.store().list("/non_primary/")` returns non-primary's
entries — the snapshot delivers correctly even from the wrong peer.
What breaks is the change-event delivery for subsequent writes.

**Scope:** affects every consumer-facing window bound to a
non-primary peer in worker mode — Entity Tree, KB, Execute Console,
Query Console, Settings. Symptoms vary (selection broken, save
flow appears stuck, dropdowns stale), all rooted in the same
"subscription registered on wrong peer" bug.

**Why we missed it:** E2E (`tests/e2e_worker.rs`) exclusively tests
primary-peer windows. Phase 5 `inspector_populated` exercises Entity
Tree on primary; the non-primary path was never covered. Manual
testing was also primary-heavy.

**Audit:** A full wire-protocol audit confirmed Subscribe is the
only variant missing peer_id (all 14 data-plane variants have it);
the host audit confirmed `handle_subscribe` is the only handler that
hardcodes primary (all other handlers correctly resolve
`sdk.peer(&peer_id)`); and the consumer-side ignore-peer_id smells
number 2 instances, only one of which is load-bearing.

**Fix path:**
- Upstream: add `peer_id` to `Request::Subscribe`, change
  `handle_subscribe` to use the request's peer_id instead of
  primary. PROTOCOL_VERSION 5 → 6. ~5 LOC mechanical.
- Consumer: thread peer_id through `proxy.observe(peer_id, prefix)`
  + `WorkerPeerStore::watch_prefix(watch, peer_id, prefix)` +
  `Peers::watch_prefix` Worker arm; remove `let _ = peer_id` line.
  ~10 LOC.
- E2E: add Phase 13 — open Entity Tree on a non-primary local peer,
  click a row, assert inspector populates. Same shape as Phase 5,
  different bound peer.

**Status:** ✅ Fixed end-to-end. PROTOCOL_VERSION 5 → 6
landed with:
- Wire: `Request::Subscribe { request_id, sub_id, peer_id, prefix }`.
- Host: `handle_subscribe` resolves the request's peer_id (falls
  back to default with deprecation warning if v5 proxy sends empty).
- Proxy: `observe(peer_id, prefix)` signature.
- Consumer: `WorkerPeerStore::watch_prefix(watch, peer_id, prefix)`
  forwards peer_id; `Peers::watch_prefix` Worker arm passes it
  through. `tree_listing`'s `let _ = peer_id` retained as cosmetic
  (cache mirror is path-keyed so per-peer disambiguation is
  natural; documented inline).
- Audit: upstream confirmed clean elsewhere — no other
  `default_peer_id` / `primary_peer_id` leaks anywhere in
  `core/`, `extensions/subscription`, `wasm-worker-proxy`, or
  the rest of `wasm-worker-host`. The Subscribe leak was the
  singular outlier.

✅ **Phase 13 E2E** opens Entity Tree on the non-primary peer created
in Phase 12, clicks a row, and asserts the inspector populates within
that section. Same shape as Phase 5, different bound peer. It locks
in v6 going forward and would have caught this bug at CI.

**Process lesson:** the "is this peer-scoped?" check should be a
reflex on every cross-peer wire surface. Upstream did its own audit
pass and confirmed Subscribe was the singular "defaults to primary"
leak — no other instances anywhere in `core/`,
`extensions/subscription`, `wasm-worker-proxy`, or the rest of
`wasm-worker-host`.

### 3.9 SDK-as-router vs peers-as-transport-aware — open design

**Question raised** while diagnosing the v6 subscribe
peer-scoping bug: we noticed that "intra-SDK peer-to-peer
communication" is happening through SDK-layer routing, not through
any actual transport between peers. Worth naming the two mental
models so the cost of switching is visible if/when the use case
demands it.

**Model A — SDK-as-router (current):**

- Peers don't know about each other.
- The SDK owns a dispatch table: `entity://{peer_id}/...` URIs
  resolve against the local peer registry first; if the target
  is in this SDK, dispatch goes through that peer's L1 directly.
  Otherwise the connection pool routes to a remote peer over a
  transport (WebSocket today, eventually WebRTC/etc. — see §3.3
  / Parity-D-narrow).
- The *initiating* peer never knows whether the target is local
  or remote. It just calls execute with the URI; the SDK figures
  out where to land.
- The application layer (us) drives routing by calling
  `peers.execute(peer_id, ...)`, `peers.dispatch_write(peer_id, ...)`,
  etc. The v6 subscribe fix completes this picture for
  subscriptions — host resolves the right PeerContext and
  registers the L1 callback there.

**Model B — peers as autonomous transport-aware agents:**

- Each peer holds a `Connection` to every other peer it talks to.
- For peers in the same SDK, the "connection" is an in-process
  IPC channel (a `Sender`/`Receiver` pair, or similar) that
  *looks* like a transport from the peer's perspective but
  bypasses the network. Implementation would be an
  `InProcConnector` impl alongside `BrowserWebSocketConnector`.
- For peers in different SDKs / processes / machines, the
  connection is a real network transport (WebSocket / WebRTC /
  whatever).
- The peer doesn't know whether its connection is in-proc or
  over-the-wire. That's the point of the abstraction.
- Peers can act autonomously — establish their own connections,
  observe each other's state, react to events — without the
  application layer routing every call.

**When Model B becomes worth the cost:**

- Peers run autonomously (e.g., agent peers reacting to each
  other without app prompting). Today the v6 fix makes
  cross-peer subscribe possible *because the app sets it up* —
  the peer itself doesn't establish anything. Autonomous-peer
  use cases would need Model B.
- Code uniformity — peer logic shouldn't have to branch on "is
  this target local or remote?" The transport abstraction
  handles it.
- Spec alignment — entity-core protocol assumes peers as
  network participants. Model A is an efficiency optimization
  for in-SDK routing; Model B honors the spec's mental model.

**Estimated cost:** ~200 LOC across `entity-peer/transport`
(new `InProcConnector` + `InProcConnection` impls) and the SDK
builder (register the in-proc connector at peer-build time
alongside whatever real connector). Plus a discovery story —
peers need to know which other peers exist in the same SDK to
auto-establish in-proc connections. Probably reads from the
SDK's peer registry at handshake time.

**Today's stance:** Model A is sufficient for "one app driving
multiple peers via UI." That's our current product surface. If
autonomous-peer use cases land, Model B becomes the natural
next architectural step. **Tracked here so we don't accidentally
build Model A assumptions deeper into the codebase that would
make migration painful later.**

**Status:** Open. Not blocking. Revisit when:

- A consumer flow requires peer A to react to peer B's state
  changes without app prompting.
- The `entity://...` dispatch boundary becomes a friction point
  in code that should be transport-agnostic.
- Cross-SDK / cross-process peer communication enters scope and
  the choice of "do the in-proc and out-of-proc transports look
  the same to peer code?" matters.

---

## 4. Decisions made

Reasoning preserved so we don't relitigate in six months.

### 4.1 Wire ships content-bearing Change events (not notification-only)

**Question:** Should `Event::Change` carry the new entity inline
(content-bearing) or just a notification + hash (notification-only,
consumer fetches on demand)?
**Decision:** Content-bearing, status quo.
**Reasoning:** Performance arguments against were largely speculative
(burst sync of 10K entities is hypothetical, realistic per-frame
churn is dozens of writes). Wire shape is already implemented and
working. Switching to notification-only would have downgraded
`cache_get` from sync to sync-or-stale, requiring all consumers to
handle that. The current shape is honest about the L1 layer's actual
output (entity + hash + kind).

### 4.2 Initial snapshot uses L0 store scan, not L1 list recursion

**Question:** How does the host populate `Event::Snapshot.entries`
for a prefix subscription?
**Decision:** Use `peer_ctx.store().list(prefix)` (L0, flat, all
descendants).
**Reasoning:** L1 `peer_ctx.list(prefix)` returns immediate children
only. For any non-leaf prefix, the listing is directory entries with
no hash, which the snapshot builder correctly skips — producing empty
snapshots. L0 store scan returns all path → hash bindings under the
prefix in one call.

### 4.3 KB drops parallel cache for tree-as-source-of-truth

**Question:** Should KB maintain `inner.articles` /
`inner.current_article` alongside the proxy cache mirror?
**Decision:** No. Single source of truth is the tree. `render_output`
reads fresh.
**Reasoning:** The dual-cache pattern can diverge — optimistic state
showed an article in the list, but `cache_get` for that path returned
None on click. User-visible bug: "selected article is no longer
available." Single source of truth eliminates the divergence class
entirely.
**References:** This document §2.1; the KB `model.rs` refactor.

### 4.4 Worker-host snapshot logic stays in the host

**Question:** Should `build_initial_snapshot`'s 5-line iterator
(list + filter_map + content lookup) be promoted to an SDK method
(`list_entities` on `StoreAccess`)?
**Decision:** Yes, eventually. Consumer-side first, but agreed it
belongs in the SDK once another consumer surfaces.
**Reasoning:** The "snapshot a subtree" primitive is conceptually
general. Being the first consumer doesn't mean being the only
consumer — a native local cache layer or another binding could reach
for the same thing. Cost of adding it to the SDK is trivial (wraps
existing primitives, no new capability). Pending upstream.

---

## 5. Migration phase status

| Phase | Description | Status |
|---|---|---|
| 0a | Tauri WebView + OPFS verification | ✅ |
| 0b | L1-mirroring macro spike | ✅ |
| 0c | Frame-time measurement | ✅ |
| 1 | Worker crates + cache + CI lanes | ✅ checkpoint passed |
| 2 | OPFS ContentStore backend | ✅ store landed; builder wiring + worker host integration landed upstream; consumer flipped `enable_opfs: true` in `InitParams`; E2E Phase 11 verifies reload-persistence end-to-end |
| 3.0 | Pilot window (Settings) | ✅ |
| 3.2 | Arm wiring | ✅ all 7 priority arms live; arms 8-9 deferred (no consumer) |
| 3.3+ | Window migrations | ✅ all 9 windows green in worker mode (see status table below) |

### Phase 3.x window status

| Window | State | Notes |
|---|---|---|
| Settings | ✅ | First pilot. Worker round-trip verified. |
| Entity Tree | ✅ | 253+ rows verified end-to-end. Click→inspect verified. No masks. |
| Knowledge Base | ✅ | Single-source-of-truth. §3.2 race closed via `Peers::put_and_wait`. E2E save→back→click verified end-to-end. |
| Event Log | ✅ | Read-only display; populated by event_log_writer's worker arm (Parity-A close-out). Phase 8 and 9 E2E verify writes land. |
| Peer Connections | ✅ | Window renders OK in worker mode. Connect routed through Parity-D-narrow (PROTOCOL_VERSION=5); pooled inside the worker; subsequent `entity://...` URIs flow through Parity-A's `proxy.execute`. Listener stays UI-gated off (browser worker can't bind, by runtime). |
| Peers (management) | ✅ | Window renders OK; uses `has_peer_context` which works in both arms. Create / Delete wired through Parity-B (PROTOCOL_VERSION=4) — E2E Phase 12 verifies create round-trip. |
| Key Manager | ✅ display (placeholder) | Pure HTML-table from cached state. Marked "not yet connected to entity-crypto" in source — placeholder content regardless of arm. |
| Execute Console | ✅ | Parity-A wired. E2E click → 400 response verified end-to-end. |
| Query Console | ✅ | E2E Phase 9 verifies Count round-trip. Full fidelity since PROTOCOL_VERSION=4 (`total`/`cursor`/per-match `entity_type` all flow through `WireQueryResults`). |

### E2E regression gates (`tests/e2e_worker.rs`)

| Phase | Asserts |
|---|---|
| 1 | Bootstrap completes, no panics. |
| 2 | All 9 window factories open without panic. |
| 3 | Settings clicks fire `dispatch_write`. |
| 4 | Entity Tree renders ≥ 1 row (catches empty-snapshot class). |
| 5 | Tree click populates inspector (catches dirty-flag class). |
| 6 | KB save→back→click does not show "no longer available" (catches dual-cache class). |
| 7 | Execute Console handler dropdown has options (Parity-A `discover_handlers_async`). |
| 8 | Execute Console click produces ← or ✗ result in event log (Parity-A `execute` + event-log writer). |
| 9 | Query Console Count click produces a `system/query count` line in event log (Parity-A `count`). |
| 10 | Query Console Find click produces a `system/query find` line in event log (Parity-A `query`). |
| 11 | Page reload preserves the KB article saved in Phase 6 (OPFS acceptance: same primary keypair, hydrated tree, article visible in respawned KB window). |
| 12 | Parity-B "New Peer" click in Peers window grows row count by 1 AND localStorage `entity_peers` gains one new line (worker create round-trip + seed persistence). |
| 13 | Entity Tree bound to a non-primary peer renders rows AND inspector populates after click (Subscribe peer-scoping regression gate — see §3.8). |
| 14 | ConnectPeer flow against a real native WS listener: spawns `entity-browser-tauri` subprocess with `ENTITY_BROWSER_AUTOSTART_LISTENER=1`, scrapes its READY line, drives Peer Connections → Connect, asserts the remote peer's short_pid appears in Connected Peers (Parity-D-narrow end-to-end regression gate). Also asserts the Tauri WebView itself logged `Frame loop started` — autostart and the WebView are independent paths, but production users want both healthy. |

---

## 6. Open questions for upstream

1. ✅ **`put_and_wait_for_cache` exposure** (§3.2) — resolved.
   Consumer-side branching pattern adopted:
   `Peers::put_and_wait` lives in this repo as a thin
   arm-branching wrapper (Direct: `ctx.put.await`; Worker:
   `WorkerPeerStore::put_and_wait → proxy.put_and_wait_for_cache`).
   No SDK-tier change; the upstream primitive on the proxy was
   exactly the right shape.
2. ✅ **Parity-A wire arms ready?** — resolved. All four
   arms (`execute`, `query`, `count`, `discover_handlers`) wired
   consumer-side. Living here in `Peers`; not upstreamed to
   `entity-sdk` since `Peers` is application-tier.
3. ✅ **Parity-B wire protocol scope** — resolved at
   PROTOCOL_VERSION=4. Decision: ship 32-byte seed bytes inline
   in `CreatePeerOk { peer_id, keypair_seed, metadata }`. The
   seed crosses the same context-isolation boundary as the
   primary peer's seed at `InitParams.primary_peer.keypair_seed`
   on every boot, so we already accept that envelope. Host does
   not retain it server-side; consumer persists via localStorage
   for reload survival. `WirePeerMetadata` mirrors
   `entity_sdk::PeerMetadata` (label / persisted /
   listen_addresses).

**All upstream questions queued here are resolved.** Follow-on wire
work in v5 / v6 (Parity-C SetMetadata, Parity-D-narrow ConnectPeer,
Subscribe peer_id) landed cleanly without further architectural
questions; see §3.5 and §3.8 for the detail.


---

## 7. Implementation history

This section records how worker mode reached its current state — the
protocol-version progression and the recurring process lessons —
without relitigating the detail already captured in §3–§5.

### Wire-protocol progression

The worker wire protocol grew through a series of additive
`PROTOCOL_VERSION` bumps, each `#[serde(default)]`-backcompatible
with the prior:

- **v2 → v3** — OPFS persistence. `InitParams.enable_opfs: bool`;
  host branches on it and calls `build_async().await` through the
  full `PeerBuilder → PeerContextBuilder → EntitySDKBuilder` OPFS
  chain. Consumer flips `enable_opfs: true` in `app.rs::new_wasm_worker`.
  Closes Phase 2 (see §5); E2E Phase 11 verifies reload-persistence.
- **v3 → v4** — Parity-B peer create/delete plus richer query
  results. `Request::{CreatePeer, DeletePeer}`, `CreatePeerOk
  { peer_id, keypair_seed, metadata }` with `WirePeerMetadata`, and
  `WireQueryResults.total/cursor` + `WireQueryMatch.entity_type`
  (previously lossy). Host generates the keypair via browser
  getrandom and returns the seed inline; consumer persists it via
  localStorage for reload survival. E2E Phase 12 verifies the create
  round-trip plus seed persistence.
- **v4 → v5** — Parity-C `Request::SetMetadata` (rename/label, wire-
  ready, no current UI call site) and Parity-D-narrow
  `Request::ConnectPeer` (outgoing connections only). The host inlines
  `Peer::connect_to`'s body to avoid holding a `&Peer` across the
  network await; the worker pools the connection so subsequent
  `entity://{remote}/...` URIs route through the Parity-A execute arm.
- **v5 → v6** — `Request::Subscribe` carries `peer_id`;
  `handle_subscribe` resolves the request's peer instead of
  hardcoding primary (with a backcompat fallback for an empty
  peer_id from older proxies). This closes the §3.8 non-primary
  subscribe bug. E2E Phase 13 is the regression gate.

The consumer-side wiring pattern for each new SDK capability is
uniform: a host handler, a proxy method, a `Peers` arm-branching
wrapper, and a cfg-gated `Send` bound (native tokio needs `Send`;
WASM `spawn_local` doesn't). New SDK features follow this pattern
without further coordinated handoffs unless they introduce genuinely
new wire shapes. From the consumer perspective the SDK is stable: the
worker wire surface now mirrors what `entity_sdk::PeerContext`
exposes, and the only remaining "panics on Worker" lines in `Peers`
are intentional Direct-only escape hatches (`peer_shared`,
`peer_context`, `sdk`, `sdk_mut`, `peer`), none reachable from
worker-mode UI flows.

### Recurring lessons

- **App-tier writers must implement both arms.** Three writers
  (`event_log_writer::log()`, `peer_registry_signal::bump()`,
  `connections::ConnectionsWriter::add()`) each shipped with a
  Worker-arm stub that compiled cleanly and silently no-op'd,
  producing a "feature does nothing in worker mode" bug each time.
  After the third instance the dual-arm branching was extracted into
  `WriterHandle` (§3.7), making the bug class unrepresentable. The
  meta-lesson: when "follow this template carefully" starts standing
  in for an abstraction, pull the abstraction out.

- **Persistence-sensitive code must be exercised in every runtime,
  not just one.** The E2E suite drives Firefox via Selenium, which
  supports worker OPFS — so it stayed green while Tauri's WebKitGTK
  WebView (which does not expose `WorkerNavigator.storage`) crashed
  on worker spawn (§3.6). The same single-runtime blind spot let the
  "no connector configured" host bug and the non-primary subscribe
  bug ship green. Cross-runtime topologies (e.g. a Tauri native
  listener plus a Firefox worker-mode client) surface failures that
  no single-runtime test sees.

- **Every cross-peer wire surface needs an explicit "is this
  peer-scoped?" check.** `Request::Subscribe` was the one variant
  that defaulted to the primary peer; a full audit (consumer,
  proxy, host, and upstream `core/` / `extensions/subscription`)
  confirmed it was the singular leak.

- **Self-initiated re-renders flow through the subscription
  pipeline, not `mark_dirty()`.** Direct `mark_dirty()` calls were
  used as masks while the L1 fan-out bug (§3.1) was open and removed
  once it was fixed. A recurrence of that symptom means the pipeline
  has regressed — that is the real bug to fix (§2.4).

### E2E coverage

The regression suite (`tests/e2e_worker.rs`) grew alongside this
work; Phases 1–14 are enumerated in §5. Phase 14 connects against a
real native WS listener by spawning `entity-browser-tauri` with
`ENTITY_BROWSER_AUTOSTART_LISTENER=1`, scraping its READY line, and
driving the Peer Connections window — the Parity-D-narrow end-to-end
gate.

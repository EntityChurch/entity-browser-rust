# Model — Browser/WASM Runtime

> **Status:** Ground-truth reference for "how does our substrate *actually*
> behave?" — the browser + WASM runtime the way `MODEL-GODOT-RUNTIME` is for
> Godot. **Read it first** before debugging any leak / lifetime / freeze /
> persistence question.
>
> **Maturity:** the **structure, the invariants (§11), and the boundary map
> (§10) are authoritative now** — they are derived from code we verified and
> bugs we shipped. The **empirical deep-fill** of several sections (exact GC
> timing, OPFS flush semantics, structured-clone cost) is **scheduled work**
> (HARDENING-DAG node **B1**). Sections needing it are marked
> **⏳ EMPIRICAL PASS PENDING**. Do not treat a pending section as verified.
>
> **Authority:** subordinate to the spec guides in AGENTS.md and to
> `DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md` (the lens). Update this
> doc when substrate behavior is verified or a new audit surfaces a gap — never
> paste an alternate explanation into a recipe; point recipes here.

---

## 0. The headline — the one fact that, forgotten, leaks or freezes

**There are two address spaces with two disciplines, and the boundary between
them is where everything goes wrong.** WASM linear memory is deterministic:
Rust `Drop`/`dealloc` runs exactly when scope ends or an `Rc`/`Arc` hits zero.
The JS heap is garbage-collected: tracing, non-deterministic, and it will
reclaim a wrapper *without* running the Rust `Drop` behind it. Anything that
crosses — a `Closure`, a retained `JsValue`, a DOM `Element` handle, a
`MessagePort` — is owned on one side and observed on the other, and that is
where leaks (`Closure::forget`), stuck borrows (the frame loop), and stale
handles (detached DOM nodes) live. Grounds discipline **D12**.

Second headline, equal weight: **there is exactly one frame loop**
(`main.rs:241-273`), and a panic inside it freezes the entire app while DOM
events keep firing. Grounds **D13**.

---

## 1. The substrate sandwich (where we live)

See `DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md` §1 for the full
double-sandwich diagram. In brief: **L5 Dom** → **entity-sdk / Peers router**
→ **the entity-OS kernel (L0–L2.5), hosted inside our own WASM** → **the
browser as userspace OS** → **host OS** (direct via browser, or native via
Tauri `src-tauri/`). The browser row is our substrate — the layer we cannot
change and must therefore model exactly.

---

## 2. Type-lifecycle matrix — who allocates, who frees, when

The question every row answers: *for this kind of object — who allocates, who
frees, when, which heap, what's the failure mode?*

| Kind | Heap / store | Freed when | Failure mode if mishandled |
|---|---|---|---|
| `Box`/`Rc`/`Arc<T>` (Rust-owned) | WASM linear | scope end / refcount→0 (deterministic) | `Rc<RefCell>` cycle → never freed (no cycle collector) |
| `Closure<dyn FnMut>` | WASM owns data, **JS owns a function-table slot** | when the `Closure` is dropped — **unless `forget()`** | `forget()` = permanent leak (AP1); dropping early → "closure invoked after free" JS error |
| `JsValue` / DOM `Element` handle | JS GC heap, refcounted from Rust | Rust handle drop releases the ref; node GC'd when unreachable from JS *and* Rust | detached node still referenced from Rust → stale handle (§9) |
| `MessagePort` / Worker | browser-process resource | port `close()` / Worker `terminate()` | not unregistered on peer delete → leaked port + routing entry (D14, AP9) |
| Subscription handle (`WindowWatch`) | Rust-owned, wraps L0 `subscribe` | dropped on window close → callback cancelled | not dropped → callback fires into a dead window (AP5) |
| Tree entity (persisted) | OPFS / IndexedDB / in-memory per arm | **only when we delete it** — store does not auto-GC | orphaned entity accumulates (D9-persistence); offline-wipe loses it (AP8) |
| Keypair | localStorage | explicit clear only | survives reload (the one durable thing in browser Direct mode) |

**Our own allocations, bucketed:** every object this app creates falls into one
of these rows. ⏳ **EMPIRICAL PASS PENDING (B1):** an exhaustive per-`new`/`put`
inventory ("every memory allocation, every peer allocation") — the user's bar
is that *nothing* is unaccounted. The skeleton above is the bucket set; B1
fills the census.

---

## 3. Process-lifetime singletons — what lives the whole session

What boots once and lives until the tab closes, in construction order
(`EntityApp` boot, `src/app.rs` + `src/main.rs`):

- `EntityApp` (the app `Rc<RefCell<…>>` held by the rAF closure).
- The **rAF callback** itself (`main.rs:242-273`) — self-referential `Rc` so it
  can reschedule itself.
- The **boot/primary SDK** (slot 0 of `Peers.sdks`) — Direct or Worker.
- `xworker_broker` (main-thread `MessagePortBroker`) + per-Worker control ports.
- App-tier writers (`event_log_writer`, `connections`, `listener_state`,
  `peer_registry_signal`) — each a clonable `WriterHandle`.

**Cache-eviction addendum (D9-router):** every per-X cache a singleton holds —
`peer_routes`, per-peer Worker SDKs, connection pools, `WindowWatch` tables,
broker routing entries — **must** have an eviction call wired from the
X-teardown seam. `unregister_peer` on the delete path is the hook. ⏳ **PENDING
(C2/B1):** verify each hook has ≥1 production caller (AP9 forcing function).

---

## 4. Reference cycles — the silent leak class

No cycle collector exists on either side. Two cycle shapes leak forever:
- **Rust-side:** `Rc<RefCell<A>>` ↔ `Rc<RefCell<B>>` mutual strong refs → use
  `Weak`.
- **Cross-heap:** a DOM listener `Closure` that captures an `Rc` back to the
  element's owner, while the element holds the listener — the classic browser
  cycle. `DomCtx`'s store-and-rebuild-frees-them pattern breaks it by dropping
  the `Closure` on each rebuild.

⏳ **EMPIRICAL PASS PENDING (B1):** audit for `Weak` usage vs strong cycles;
expected result "zero unbroken cycles, forward-watch," proven by grep + review.

---

## 5. Teardown timeline — what runs, in what order, when

The browser has no `queue_free` end-of-frame contract. The closest determinism:
- Dropping a `Closure` synchronously frees its WASM data and invalidates the JS
  slot.
- Removing a DOM node is synchronous for the DOM; the node's memory is GC'd
  later (non-deterministic); the node's listener `Closure`s are freed only when
  Rust drops them (DomCtx rebuild), **not** when the node is removed.
- `WindowWatch` drop cancels the underlying subscription synchronously.
- The deferred boundary is the **microtask/macrotask + rAF tick** — async
  `dispatch_write` lands across a tick (the resolver's `Pending`/repaint seam
  absorbs the delay — this is why the worker-arm `ensure_demo_site` fix works).

⏳ **EMPIRICAL PASS PENDING (B1):** the precise ordering of element-removal vs
listener-drop vs GC, and where we rely on it.

---

## 6. The WASM↔JS boundary — wasm-bindgen seam

The FFI seam and its `Drop` semantics:
- `JsValue` is refcounted from Rust; the Rust handle drop releases one ref.
- `Closure` ownership: hold it (store in `DomCtx`) or it's dropped (slot
  invalidated). `.into_js_value()` / `.forget()` deliberately leak — **never in
  non-test code** (D12 grep).
- **The hard rule (the wasm-bindgen analogue of Godot's `Base<T>` one-frame
  defer, but worse — unbounded):** JS GC of a wrapper does **not** promptly run
  Rust `Drop`. Never rely on JS dropping a wrapper to run Rust cleanup.
- What crosses the seam: DOM handles, event payloads, `MessagePort`s,
  structured-clone'd CBOR over `postMessage`.

⏳ **EMPIRICAL PASS PENDING (B1):** wasm-bindgen's documented GC-timing
guarantees (or lack thereof) — flag as a "docs-silent" item (§13).

---

## 7. Callback / listener lifetime

`addEventListener` + `Closure`: **removing the DOM node does not free the
`Closure`** — Rust still owns it. The `DomCtx.closures` store + free-on-rebuild
pattern IS the answer (`../guides/DEVELOPER-GUIDE.md` "DOM Event Handlers"). The capture-leak class:
a closure capturing an `Rc`/`Arc` keeps the whole graph alive as long as the
listener is registered (the wasm analogue of Godot's `bind()` leak). Safe shape:
short-lived captures freed on the next rebuild; wrong shape: `forget()`.

---

## 8. Refs held inside collections

`HashMap`/`Vec` of `Rc`/`Closure`/handles (`peer_routes`, `DomCtx.closures`,
broker routing map): `remove`/`clear` drops deterministically — **the leak is
forgetting to call `remove` on peer/window close, not the map misbehaving.**
This is D9-router restated at the collection level.

---

## 9. Liveness checks for non-owning handles

DOM: `node.isConnected` / `document.contains(node)` — a Rust-held `Element`
handle may point at a node already removed from the tree. Worker: is the port
still open. The *question* (is this borrowed handle still live before I touch
it?) is identical to Godot's `is_instance_valid()` even though the mechanism
differs.

---

## 10. The boundary map — where bug-classes live (AUTHORITATIVE)

Each layer-crossing: the **currency** (data shape that crosses), the **seam**
(code location), the **bug-class** empirically located there.

| Boundary | Currency | Seam | Bug-class |
|---|---|---|---|
| **A. DOM → app** | `Action` enum | `DomCtx` → `process_actions` (`src/action.rs`, `src/app.rs`) | wrong-peer routing; sync-vs-async action dispatch |
| **B. WASM ↔ JS** | `JsValue` / CBOR / `Closure` | wasm-bindgen | `Closure::forget` leak (AP1); stale DOM handle; relying on JS GC for Rust drop |
| **C. main ↔ Worker** | `postMessage` + `MessagePort` + transferables | `xworker_broker` / `WebTransport::with_control_port` (`src/peers.rs`) | port lifecycle (D14); init-message race; unregister-on-delete (AP9) |
| **D. app ↔ kernel** | L0 sync `store()` / L1 dispatched `get/put` | `PeerContext` (`Peers::peer_context*`) | L0 security opt-out (D2); **defaults-to-primary peer-scoping (AP2)**; spec drift (AP6) |
| **E. arm split** | Direct (main thread) vs Worker (process) | `Sdk::Direct` / `Sdk::Worker` (`peers.rs:378-394`) | **Direct-only API on Worker arm → panic → freeze (AP3/AP4)** |
| **F. app ↔ store** | CBOR entity → OPFS/IDB/in-mem | `persistence.rs`, OPFS tombstone path | offline-wipe (AP8); WebKitGTK OPFS-gap; orphan accumulation (D9) |

This table is the single most useful artifact for triage: a freeze/leak/lost-
state report maps to a boundary, and the boundary names the likely discipline
violated.

---

## 11. Invariants — the checklist the whole doc reduces to (AUTHORITATIVE)

Each is *[mechanism] → [discipline it forces]*. This is the portable checklist;
every other section justifies one line.

1. **Two heaps** — WASM linear memory is deterministic (Rust `Drop`); JS heap
   is GC'd (non-deterministic). Account for both. [D12]
2. **Two cleanup modes** — Rust-owned (drop on scope/refcount→0) vs JS-retained
   (`Closure` you store-and-drop). [D12]
3. **`Closure::forget()` is a permanent leak.** [D12, AP1]
4. **DOM listeners are freed by dropping the stored `Closure`, not by removing
   the node** — rebuild-frees-them (`DomCtx`). [D12]
5. **No cycle collector** — break `Rc`/JS↔WASM cycles with `Weak`/explicit drop.
   [D12]
6. **Never rely on JS GC of a wrapper to run Rust `Drop` synchronously.** [D12]
7. **One frame loop; a panic in `frame()` freezes the app** while DOM events
   keep firing. Isolate per-frame. [D13, AP3]
8. **Per-peer / per-window caches need an eviction call at the close seam**
   (`peer_routes`, subscriptions, pools, ports). [D9, AP9]
9. **Cross-Worker handles have explicit lifecycle** — register on attach,
   unregister on delete; transferables move, not copy. [D14]
10. **Persistence outlives the process, fails silently, AND is evictable** —
    durable-on-disk ≠ permanent. OPFS sync handles are **Worker-only** (the
    reason Worker mode exists); even durable storage is **best-effort and evicted**
    (Safari ~7-day ITP purge + LRU under pressure) **until `navigator.storage.persist()`
    is granted** — which we now request at boot (C5a,
    `storage_durability::request_persistent_storage`), but the grant can be
    *denied*, so a durable tree stays best-effort until then (the C5d evictable
    banner is the honesty). Tombstone + durability discipline. [D16, AP8]
11. **The arm is per-peer, decided by the target peer's `peer_context`, never
    the primary.** [D15, AP4]
12. **No cross-origin isolation → cross-Worker transfer is copy-cost structured
    clone; `SharedArrayBuffer` / WASM threads are off the table.** Enabling them
    needs COOP/COEP headers, which forfeit cross-origin embedding (third-party
    iframes/scripts) — a decision we have *not* made (verified: no SAB, no
    COOP/COEP — `src/capabilities.rs`). So lean/batch the wire; don't design for
    shared memory. [D14]
13. **Never cache a JS-held WASM memory view (`Uint8Array` over
    `memory.buffer`) across an allocating call.** A `Vec`/`String` growth can
    trigger `memory.grow()`, which *detaches* the view (wasm-bindgen#4395) — the
    next read throws or reads garbage. Re-fetch `memory.buffer` after any call
    that can allocate, or don't hold the view. (wasm-bindgen marshals for us, so
    this bites only hand-rolled glue.) *Audited: zero held views in
    `src/` — no `memory.buffer` / `Uint8Array`-over-memory / `ImageData` paths;
    QR is SVG, barcode is the native `BarcodeDetector`.* [D12]
14. **OPFS is single-writer and the browser does NOT coordinate it for us.**
    The sync access handle is exclusive per file; **Web Locks does not lock OPFS
    files** — same-origin single-writer coordination is *ours*. Two tabs share
    one keypair → one peer_id → one OPFS root, so the second tab must be detected
    and held off (the multi-tab guard, `src/multitab.rs`, is the enforcement
    point — a Web Locks leader election keyed on the peer_id). [D16, D9]

---

## 12. Anti-pattern cross-reference

The catalog lives in the charter (§5, AP1–AP11). Each anti-pattern is anchored
to a real incident; this doc supplies the *mechanism* behind each (§§2–11).

## 12.5 Featured gotcha — the rAF freeze (full treatment)

The substrate quirk gnarly enough to earn a full section, because it cost a
multi-hour misdiagnosis (AP3).

**Layout.** `main.rs:241-273`: a self-`Rc` `Closure` runs `app.frame()` then
reschedules via `request_animation_frame` (`268-272`). A `try_borrow_mut` guard
(`252-257`) logs `"FRAME SKIP — app RefCell already borrowed"` on double-borrow.

**Wrong (current, partial):** the guard catches a *stuck borrow* but a raw
panic inside `frame()` unwinds past the reschedule → loop dead. Under the
dev/abort profile (`panic = "unwind"` is **release-only**, `Cargo.toml:230`)
the panic aborts regardless. DOM events keep firing → looks like a connect/
timing bug, not a freeze.

**Right (node C1):** reschedule *before* the fallible section, and/or set
`panic = "unwind"` on the dev profile + wrap `frame()` in
`catch_unwind(AssertUnwindSafe(..))` that logs **loudly** (distinct error
marker; the panic hook still fires so e2e `count_panics` catches it) and
continues. **Never silently swallow.**

**Verification gate:** an e2e phase that spawns a window known to panic and
asserts (a) the panic is logged and (b) the frame loop survives (window count
keeps advancing). Plus the Direct-browser e2e (handoff §5) that would have
caught the original freeze.

---

## 13. Docs-silent items — now researched

The substrate-research passes resolved most of these:
- **Browser storage substrate** — OPFS worker-only,
  durability/eviction (Safari 7-day ITP + LRU + `persist()`), quotas. ✅
- **JS↔WASM boundary** — wasm-bindgen GC-timing (WeakRef/
  FinalizationRegistry), the CBOR copy-by-clone Worker wire, no-SAB-by-design,
  binary size. ✅
- **Browser config matrix** — per-engine support
  (iOS=WebKit), BarcodeDetector Chromium-only, WebKitGTK gap. ✅

**Still genuinely open:** exact
WebKitGTK OPFS version; iOS WASM-memory ceiling vs our bundle size; SIMD in our
release build; `MessageChannel`-vs-rAF delivery ordering (low priority).

---

## 14. How to use this doc

- **Code review:** is this Rust-owned or JS-retained? Is a `Closure` stored or
  `forget()`'d? Does it cross the Worker boundary (register/unregister)? Does the
  op route by `peer_id` or default to primary? Which arm?
- **Debugging a freeze:** grep `panicked at` in `window.__entity_browser_log`;
  find the window the count stalls at (handoff §1 method); it's a frame panic
  (§12.5) until proven otherwise — *not* the substrate (AP6).
- **Debugging a leak:** map it to a boundary (§10); check the matching invariant
  (§11).
- **Debugging lost state:** which store (§2 row F)? what's its durability +
  fallback (D16)? did offline-wipe or the WebKitGTK OPFS gap hit?
- **Architecture decision:** which layer, which heap, which arm, which store —
  the four substrate axes.

---

## 15. Changelog
- Structure + invariants (§11) + boundary map (§10) + rAF featured-gotcha
  (§12.5) are authoritative; §§2–9, 13 deep empirical fill is scheduled work.
- §11 invariants 12 (no cross-origin isolation → structured-clone copy cost,
  SAB/threads off the table without a COOP/COEP forfeit), 13
  (detached-ArrayBuffer-after-grow), and 14 (OPFS single-writer is ours — Web
  Locks doesn't lock OPFS files; the multi-tab guard enforces it). Invariant
  10 reflects that `persist()` is now requested at boot.

# Discipline Reframe — Browser/WASM Substrate

> **Status:** Discipline charter for **Dom** (this repo, `entity-browser-rust`).
> Supersedes the working framing of this project as "the DOM frontend POC
> for the entity system." Going forward this project is **an L5 application
> running on two stacked userspace operating systems — the browser (the
> W3C sandbox) for display/input/storage/processes, and the entity-system
> kernel (L0–L2.5) for state/dispatch/capability — and our discipline is to
> act like that.**
>
> **Why now.** Dom started as a proof-of-concept. It is now the **flagship
> deployment** of the entity system: the web/Tauri path is the one users
> actually hit, and Godot is months behind on native and has no web path.
> The POC framing has to end. This charter is the end of it.
>
> **Provenance.** This is the browser-substrate translation of Godot's
> discipline arc (the "EOS reframe"): the Godot reframe, runtime model,
> core reference, roadmap DAG, drift audit, foundation retrospective, and
> workbench-dev guide (all in `../godot-entity-core-rust/docs/`).
> **We do not copy their substrate.** Their middle OS layer is Godot 4; ours
> is the browser. The *method* — name the sandwich, mine the OS/web-platform
> canon for convergent rules, map each rule onto a named seam, turn each into
> a review question — is identical. Only the substrate row changes. Where
> their rule is about the shared entity-OS layer it transfers verbatim; where
> it is about Godot internals we replace it with the browser equivalent.
>
> **Read order:** this charter first (the lens). Then
> `MODEL-BROWSER-WASM-RUNTIME.md` (how the substrate actually
> behaves — read it first for any leak/lifetime/persistence question), then
> the forward navigation surface and the mode/config/posture grounding this
> charter sits on.

---

## 0. The reframe in one paragraph

"We are building an operating system, not an application" is literal here,
not metaphor — the entity-core papers (DEOS / Paper 7) already say it: handlers
are processes, the tree is a filesystem, extensions are kernel services,
capabilities are the authorization surface. We are an **L5 application** on
that distributed OS. But unlike Godot, our app does not run on a native
toolkit — it runs **inside a second OS we did not write and cannot change:
the browser**. The browser is a real userspace operating system for the web
sandbox: the event loop + `requestAnimationFrame` is its scheduler, the DOM
is its display server, the Web APIs are its kernel services, Web Workers are
its processes, `postMessage`/`MessagePort` is its IPC, OPFS/IndexedDB/
localStorage are its persistent stores, and WASM linear memory + the JS GC
heap are its **two address spaces**. The discipline is to recognize this
**double sandwich**, name each layer's contract precisely, and stop drifting
between "we are a UI toolkit user" and "we are an OS builder." The web
platform's hard constraints — non-deterministic GC, the single frame loop,
the WASM↔JS boundary, opaque storage durability, per-origin isolation — are
not annoyances to paper over; they are the substrate's contracts, and every
one of them has already bitten us (§3). We name them or we ship a pile.

---

## 1. The double-sandwich substrate

```
┌───────────────────────────────────────────────────────────────────────┐
│ L5 — Dom (this repo)                                                    │
│      WindowManager, DOM views (render_dom), Action dispatch, DomCtx,    │
│      WindowWatch reactivity, app_paths namespace, persistence boundary  │
├───────────────────────────────────────────────────────────────────────┤
│ L4 — SDK shared patterns (entity-shell, GUIDE-ENTITY-WORKBENCH-APP)     │
│ L3 — entity-sdk facade: Peers router (Vec<Sdk>, peer_routes),           │
│      PeerManager / WorkerPeerStore, PeerContext, subscription L1 prims   │
│ L2.5 — substrate-bridge extensions (tree, content, sub, continuation,   │
│        query, revision, history, …)              ← THE DEOS KERNEL      │
│ L2  — SYSTEM-COMPOSITION                                                 │
│ L1  — core protocol (EXECUTE, capabilities, primitives)                 │
│ L0  — algorithm library (CBOR/ECF, SHA-256, Ed25519)                    │
│   ↑ this entire stack runs INSIDE our WASM module(s) — main thread      │
│     (Direct arm) and/or dedicated Web Workers (Worker arm, per peer)    │
├═══════════════════════════════════════════════════════════════════════┤
│ BROWSER — userspace OS for the W3C sandbox                              │
│   scheduler ............ event loop + microtask queue + rAF tick        │
│   display server ....... DOM + CSSOM + Shadow DOM (style isolation)     │
│   kernel services ...... Web APIs (storage, crypto.subtle, WebSocket,   │
│                          BarcodeDetector, structuredClone, …)           │
│   processes ............ Web Workers (boot worker + per-backend workers)│
│   IPC .................. postMessage / MessageChannel / MessagePort     │
│   persistent store ..... OPFS · IndexedDB · localStorage (+ in-memory)  │
│   address spaces ....... WASM linear memory (deterministic) ‖           │
│                          JS GC heap (non-deterministic)                 │
│   the wire ............. structured clone + transferables               │
├───────────────────────────────────────────────────────────────────────┤
│ HOST OS (Linux/Wayland here) — reached only through the browser, OR     │
│   through Tauri's WebKitGTK WebView + native src-tauri backend (IPC)    │
└───────────────────────────────────────────────────────────────────────┘
```

**Two userspace OSes, stacked.** We sit at L5 and ride both: the entity-OS
kernel (L0–L2.5, which *we host* inside our own WASM) and the browser (which
*hosts us*). The host OS we touch only through the browser, except in Tauri
where `src-tauri/` reaches it natively for backend peers.

**This is the one substitution from Godot's reframe.** Their middle OS row
was "Godot 4 — MainLoop / Servers / RIDs / SceneTree / autoloads / signals."
Ours is "Browser — event loop / Web APIs / Workers / postMessage / OPFS / two
heaps." Every discipline that lives at L0–L4 transfers verbatim (same kernel).
Every discipline that lives in the substrate row is **re-derived for the
browser** (§ D12–D16).

**The payoff is the same: it ends a class of confused conversations.** An
ambiguous "is this X-side or Y-side?" becomes answerable by *which layer*:

| Vague question | Becomes |
|---|---|
| "Store this in a Rust field or the tree?" | "Is this L5 session scratch or kernel state?" |
| "L0 `store()` or L1 `get()/put()`?" | "Internal bookkeeping, or could another peer observe it?" |
| "Hold this `Closure` or `forget()` it?" | "Which heap owns it, and what's the drop path?" |
| "Run this on the main thread or a Worker?" | "Which browser process should own this peer's SDK?" |
| "Is this freeze a substrate bug?" | "Which layer's contract did we violate?" (§3, AP6) |

---

## 2. The canon we inherit from (method, not copy)

Godot mined fifty years of OS design (Plan 9, Inferno, seL4, Fuchsia, BeOS,
Genode, NixOS, Urbit; X11/AppleEvents as negative examples) and kept the
rules that **converge across all of them**: bounded interfaces,
capability-typed handles, owned/reversible state machines, declarative
composition, per-principal namespaces, the kernel survives misbehaving apps.
Those are entity-OS-layer rules — they transfer to us unchanged (D1–D11).

**Our substrate adds a second canon Godot never had to read: the web
platform.** The convergent rules of the browser/WASM runtime canon —
distilled from the engines and frameworks that fought these exact battles —
give us D12–D16:

- **V8 / SpiderMonkey GC + WASM linear-memory model** → two address spaces,
  one deterministic and one not; the boundary is where leaks live. → **D12**.
- **wasm-bindgen / Emscripten FFI discipline** → `Closure` ownership, the
  "JS GC of a wrapper does not run Rust `Drop`" rule. → **D12**.
- **The single-threaded event loop + rAF render contract** (every browser
  game/engine loop) → one frame loop, never let a frame kill it. → **D13**.
- **Service-Worker / Worker / `MessagePort` process+IPC model** → per-peer
  process ownership, explicit port lifecycle, transferables move not copy. →
  **D14** (and underlies the arm model, D15).
- **The storage canon (OPFS / IndexedDB / Cache API / localStorage)** →
  durability is per-store and per-engine, fallbacks are silent, "what
  survives a cold return" is a design property not an accident. → **D16**.
- **Elm/React reconciliation discipline** (subscribe to derived state, render
  is a pure function of state, no manual change-detection) → already ours
  (WindowWatch, no hashing) — see §6 "what stays."

The move is the same as Godot's: **survey the canon, keep the convergent
rules, attribute each to its source, map each onto a named seam in our code,
turn each into a review question.** We are not inventing — we are inheriting.

---

## 3. The disciplines

Eleven inherited from the entity-OS layer (D1–D11, Godot's set, re-grounded
in *our* enforcement points and *our* bugs), five native to our substrate
(D12–D16). Each is **an invariant the code obeys, not a strategy**. Each has
a **WHY** (with canon/incident provenance) and a **HOW** (with at least one
concrete enforcement point — a file, a lint, or a test gate). A discipline
with no enforcement point is theater.

Disciplines are **promoted on evidence, not speculation.** D12–D16 are
promoted because each names a bug class that has *already shipped* in this
repo (cited inline). New disciplines start as **Pending** until a bug earns
them.

### Inherited from the entity-OS layer (transfer verbatim from Godot)

**D1 — Use the kernel; stop reinventing what extensions provide.**
*Why:* the substrate-bridge extensions run inside our process; routing a
capability to its native service is "using infrastructure we already pay
for." We use ~30% of the SDK surface (`[[feedback_sdk_is_the_substrate]]`).
*How:* reactivity → subscription (already done: WindowWatch); audit →
history; versioning → revision; indexed lookup → query (not client-side
filtering); long-running ops → continuation. *Off-kernel (stays L5):*
per-frame UI scratch, DOM composition, input, theme.

**D2 — L1 dispatch is the default; L0 is the back door.**
*Why:* `ctx.store()` bypasses the capability check and dispatch chain; "every
`store()` call is a visible opt-out from the security boundary" (AGENTS.md).
*How:* L0 reserved for render-loop reads + boot bootstrap + internal session
scratch no peer will observe. Anything observable (selection, layout,
settings, roster) goes L1.

**D3 — Capability-typed dispatch (surface now, even permissive).**
*Why:* seL4/Fuchsia invariant — every privileged op needs a named cap;
retrofit is cheap now, hard later. *How:* keep the held-cap set explicit on
dispatch; fail closed. (Today every check passes; the surface is the point.)
Relevant live drift: worker-arm `subscribe` returning `CapabilityDenied` for
`system/*` (handoff §6.C) is the cap surface tightening upstream — we must
hold the cap to observe our own `system/*`.

**D4 — Bounded interfaces; one channel does one thing.**
*Why:* the anti-pattern is one channel conflating concerns (X11, AppleEvents,
a global "tree changed" broadcast). *How:* `Action` carries only actions;
selection goes through the panel-selection-source sink
(`[[project_panel_selection_source_design]]`); tree changes go *per-prefix*
through `ctx.store().subscribe`, never a global broadcast. The Phase-4 removal
of `compute_legacy_hash` was this discipline; do not reintroduce a global
generation snapshot.

**D5 — Declarative composition; boot deps are declared, not folk knowledge.**
*Why:* ordering bugs are dependency-graph violations without a graph.
*How:* boot order (`EntityApp` construction, worker spawn, broker
registration) should declare what it requires and fail loud on a missing dep,
not silently at first use. The worker init-message race
(`[[project_worker_init_race]]`) is what this discipline prevents.

**D6 — Per-host namespaces, formalized.**
*Why:* Plan 9 — namespace is per-process, lookups local, cross-namespace
access is an explicit mount. *How:* `app_paths` owns the `app/entity-browser/…`
namespace (never bake it into the SDK); windows bind by `peer_id`; the full
qualified path (with peer_id) IS the data model — never strip it.

**D7 — The kernel keeps working when applications misbehave (and vice-versa).**
*Why:* the cardinal OS rule, with a symmetric prime — the app also doesn't
assume the kernel rescues it. *How:* a panicking window must not freeze the
app (→ D13, our #1 violation, AP3); every `subscribe` unsubscribes on window
close (WindowWatch drop); every push has a matching pop. The browser will not
clean up after us.

**D8 — Trust the spec; surface drift, don't normalize it.**
*Why:* the multi-impl ecosystem coheres only if every layer's contract holds;
unspecified-but-observed behavior is drift to flag, not contract to bake in.
*How:* read code *against* spec; file dated `QUESTIONS-FOR-ARCHITECTURE`
entries (observation / spec-reading / hypothesis / what-we-did-meanwhile /
ask); **don't stall** — record a working position and proceed. Cite the
canonical source with file:line and a *type* (`[[feedback_cross_repo_citations_need_type]]`).
Its failure mode is AP6 (borrowed framing).

**D9 — Accounting: nothing accumulates that we didn't choose.**
*Why:* Godot shipped 28 phantom resources at 104/104 green; the user's frame:
*"every block of memory, every bit that goes through the system… nothing
accumulates, we know it, and when it does we know why and it's because we
chose it."* For us this has **three halves** (we own three address spaces of
state):
- *D9-runtime (the two heaps):* see **D12** — every `Closure`, DOM listener,
  `Rc`/`Arc` cycle, and cache has a documented drop path.
- *D9-persistence (the tree we write):* every persisted entity has
  **writer (single owner) / reader-at-boot / GC-story** OR a recorded
  exemption. "Our cleanup is the only cleanup" — the store does not auto-GC.
  The OPFS-tombstone delete path is this discipline working
  (`[[project_persistence_offline_wipe_bug]]` is it failing — AP8).
- *D9-router (per-peer caches):* `peer_routes`, per-peer Worker SDKs,
  connection pools, control ports, WindowWatch tables each need an eviction
  call at the peer/window-close seam. `unregister_peer` in the delete path is
  the hook — verify it is *actually called* (a defined-but-uncalled hook is
  AP9).

**D10 — Real-loop coverage. Green tests ≠ a working app.**
*Why:* the Content Site freeze (§AP3/AP4) was invisible to 331 native + 17
peer-integration + Worker-e2e green, caught only by driving the real Direct
build (handoff §1). "Internal signals lied; external evidence did not."
*How:* load-bearing changes need (1) **cross-reload** coverage (boot → act →
reload → assert state respects each store's contract); (2) the right
**startup mode** (Direct-browser is currently e2e-blind — handoff §5); (3) the
right **WebView runtime** (Firefox-green ≠ WebKitGTK-green —
`[[feedback_test_each_webview_runtime]]`); (4) **exercise the feature**, not
just spawn the window (`[[feedback_e2e_must_exercise_new_features]]`).

**D11 — Inventory-boundary declaration (meta-discipline).**
*Why:* "inventory-driven audits find what's in the inventory." *How:* every
audit/review names what's in scope AND what's explicitly NOT; the close
carries the un-inventoried domains forward; a finding from outside the
boundary extends the boundary next time.

### Native to our substrate (the browser canon — Godot never needed these)

**D12 — Two-heap accounting: WASM memory is deterministic, JS is not.**
*Source:* the V8/SpiderMonkey GC model + wasm-bindgen FFI discipline.
*Why:* Rust `Drop` runs deterministically inside WASM linear memory; anything
reachable from JS (a `Closure`, a retained `JsValue`, a DOM handle) lives on
the GC heap and is reclaimed non-deterministically — or never. **`Closure::forget()`
is a permanent leak (AP1);** JS GC of a wrapper does **not** run Rust `Drop`
synchronously, so never rely on it for cleanup.
*How:* every `Closure` is stored in `DomCtx.closures` and freed on DOM rebuild
— never `forget()` (AGENTS.md anti-pattern, enforced by the `DomCtx` helpers
`on_window_event`/`on_action`/`listen`). Break `Rc<RefCell>` / JS↔WASM cycles
with `Weak`. This is D9's runtime half, promoted to its own discipline
because the substrate makes it load-bearing. *Enforcement:* grep for
`Closure::forget` and `.into_js_value()` in non-test code → must be zero or
annotated.

**D13 — Frame-loop integrity: no window may kill the rAF loop.**
*Source:* the single-threaded event-loop + rAF render contract every browser
engine obeys. *Why:* there is exactly one frame loop (`main.rs:241-273`); a
panic in `app.frame()` unwinds past the reschedule (`main.rs:268-272`) and the
app freezes forever while DOM events keep firing — which is *precisely* why
the Content Site panic masqueraded as a connect/timing failure for hours
(AP3). A current `try_borrow_mut` guard (`main.rs:252-257`) catches the
stuck-borrow cascade but **not** a raw panic in `frame()`, and under the
dev/abort panic profile (`panic = "unwind"` is release-only, `Cargo.toml:230`)
the panic is fatal regardless.
*How:* the frame loop must be panic-resilient — reschedule *before* the
fallible section, and/or `catch_unwind(AssertUnwindSafe(..))` under a
dev-profile `panic = "unwind"`, logging **loudly** (error level, distinct
marker; the panic hook still fires so the e2e `count_panics` still catches it).
**Never silently swallow** — a frozen-but-logging app is recoverable, a silent
limp is worse than a crash. *Enforcement:* handoff §6.A; the fix is roadmap
node C1 (release blocker).

**D14 — Worker/IPC discipline: processes and ports have explicit lifecycle.**
*Source:* the Worker + `MessagePort` model. *Why:* a Worker lives until
`terminate()`; a `MessagePort` is registered/unregistered explicitly;
transferables *move* (the sender loses them); the init message can race the
worker's `onmessage` install (`[[project_worker_init_race]]`). *How:* every
peer in an attached Worker registers against `xworker_broker` on attach and
`unregister_peer`s on delete (the cross-Worker reachability work); the loader
buffers init messages and replays after wasm init; each per-peer connector
bakes in its source identity (no closure-capture). See the transport-stack
table in `IMPLEMENTATION-ARCHITECTURE.md`.

**D15 — Arm-correctness: never decide a per-peer arm from the primary.**
*Source:* our own multi-SDK router (`[[project_peer_sdk_arm_model]]`) — a
browser-substrate consequence (Direct = main thread, Worker = a browser
process). *Why:* the arm is **per-peer**, decided by the *target* peer's
owning SDK; Direct-only APIs (`sdk()`, `peer_shared`, sync `delete_peer`) brick
the Worker arm when reached unconditionally. This is the **footgun class** that
froze the app (AP4). *(The worst offender, `peer_context_or_default` — which
`panic!`ed on Worker AND silently fell back to primary — has been deleted,
closing it at the type level.)*
*How:* decide the arm from the bound peer's
`peer_context` (`Some` only on Direct, `peers.rs:378-384`), never from the
primary via `as_direct().is_none()`. Twin lifecycle ops route by the target
peer's SDK. *Enforcement:* grep all non-test callers of the Direct-only APIs;
each must be Direct-only by construction or arm-guarded (roadmap node C2,
release-blocker audit). Consider converting the `panic!` arms to
`Result`/`Option` so misuse is a compile/graceful error, not a freeze (D-track
refactor).

**D16 — Persistence-durability honesty: know what survives, where, and the fallback.**
*Source:* the OPFS/IndexedDB/localStorage storage canon. *Why:* durability is
per-store and per-engine and **fails silently**: WebKitGTK ≤2.52 lacks
`WorkerNavigator.storage` so worker-OPFS silently in-memories (→ Tauri forced
Direct, `[[project_tauri_webview_strategy]]`); a hard refresh while the server
is unreachable currently *wipes* local state (`[[project_persistence_offline_wipe_bug]]`,
AP8); browser-mode tree persistence is in-memory only today (localStorage
holds keypairs only). The user's acceptance test: *"I leave, I come back three
weeks later, did it save my shit?"* *How:* for every store we touch, document
durability + fallback + the cold-return story (the MODEL doc §persistence
pass); never gate work on a false "WASM has no filesystem" claim — OPFS/
IndexedDB *are* filesystems (`[[feedback_wasm_has_filesystem]]`); persistence-
sensitive code is tested in **each** WebView runtime (D10).

**Pending:** D17 (Application Knowledge — the model→output→renderer/T3
discipline as a first-class rule once we re-confirm where it pays off).

---

## 4. The review questions (run on every diff)

The disciplines' enforcement surface — short enough to run every change. Six
inherited, three substrate-native.

1. **Which layer is this?** L5 / L4 / L3 / L2.5-kernel / browser-OS / host-OS.
   If the layer isn't obvious, the code is confused about its place.
2. **What kernel service does this consume / reimplement?** If we reimplement,
   name it and justify (D1).
3. **What's the capability surface?** Privileged op gated, held-cap set
   explicit, fails closed (D3).
4. **Failure mode if the kernel misbehaves AND if this code misbehaves?**
   Symmetric (D7).
5. **What's the accounting?** Every `Closure`/listener/`Rc`/cache add → drop
   path identified at the same change; every persisted entity → writer /
   reader-at-boot / GC story; every per-peer cache → eviction at close
   (D9, D12).
6. **Does the test cross the real loops?** Cross-reload, real-store, the right
   **mode** (Direct *and* Worker), the right **runtime** (WebKitGTK too) (D10).
7. **Which arm?** Is any Direct-only API reached without guarding via the
   bound peer's `peer_context`? (D15)
8. **Can this panic in a frame?** If so, does it kill the rAF loop? (D13)
9. **What persists, where, with what fallback and cold-return story?** (D16)

> Started as Godot's six; grew to nine when the substrate disciplines were
> promoted. The list IS the disciplines.

---

## 5. Anti-pattern catalog

Each entry: name · the real incident that earned it · the discipline that
owns it. **The bug is what makes the rule non-negotiable** — every one of
these shipped in this repo.

- **AP1 — `Closure::forget()` permanent leak.** Forgets a JS function-table
  slot forever. [D12] *Incident: the standing AGENTS.md prohibition; `DomCtx`
  exists to make it unnecessary.*
- **AP2 — Defaults-to-primary peer-scoping.** A cross-peer wire surface
  silently using `primary_peer_id`. The `Subscribe` leak broke
  every non-primary peer in worker mode *for months* because tests only
  covered primary; the §4.3 query/count/execute hard-code is the reachable
  residual. [D2, D15, `[[feedback_audit_peer_scoping]]`]
- **AP3 — A single frame panic freezes the whole app.** Content Site panicked
  in `frame()`; the rAF loop died; every downstream e2e failure was collateral
  and the diagnosis took hours. [D13, `[[feedback_frozen_app_is_a_frame_panic]]`]
- **AP4 — Arm-split: a Direct-only API on the Worker arm.** `ensure_demo_site`
  called `peer_context_or_default().store().put()` (then `peers.rs:392`, a
  `panic!` on the worker-backed primary). *The offending method has been
  deleted — the incident is closed at the type level, but the rule stands
  for the remaining Direct-only APIs (`sdk()`/`peer_shared`/sync `delete_peer`).*
  [D15, `[[project_peer_sdk_arm_model]]`]
- **AP5 — Add without paired remove.** A `subscribe`/connection/`add_child`
  without its teardown identified at the *same* change. WindowWatch-drops-on-
  close is the right shape; a missing `unregister_peer` on delete is the wrong
  one. [D9]
- **AP6 — Borrowed framing.** Plan/handoff text propagated across sessions as
  fluent prose never re-grounded against code. The ":2918 substrate broke
  connect" misdiagnosis was this; so was the stale §6.A rAF description we just
  caught (the `try_borrow_mut` guard already exists). Fluency ≠ verified.
  [D8, `[[feedback_borrowed_framing]]`]
- **AP7 — Green-suite blindness.** 331 native + Worker-e2e green while the
  Direct-arm app froze. [D10, `[[feedback_verify_user_facing_surfaces]]`]
- **AP8 — Tree not durable outside Worker mode (was mis-framed as
  "offline-wipe").** Worker mode
  IS durable (OPFS flush-on-write); **Direct (the auto-fallback) and Tauri keep
  the tree in-memory and lose it on every reload** — the real north-star
  violation. The "offline hard-refresh wipes state" is partly a mislabel: for
  Direct/Tauri nothing was durable to wipe; for Worker the OPFS tree survives
  and the only offline risk is the asset server being unreachable so the app
  can't re-boot. Real fixes: durable Direct/Tauri (F1/F2) + offline-tolerant
  asset loading (F3). [D16, `[[project_persistence_offline_wipe_bug]]`]
- **AP9 — Defined-but-uncalled cleanup primitive.** An eviction/`unregister`
  hook that exists but no production teardown path calls — a "half-discipline."
  [D9] *Forcing function: every eviction primitive's production call sites are
  reviewed at PR time; zero callers = a violation or a tracked deferral.*
- **AP10 — Latent infrastructure rots.** Infra added for a use case without a
  test exercising it in the same commit (the "unused param for months" class).
  [D5, D10]
- **AP11 — Defensive code that lies.** Unconditional error-log on a failure you
  have a fallback for — masks real errors. Decode-fallbacks are handled, not
  error-level. [D8]

---

## 6. Naming, decision, and "what stays"

**Naming discipline.** OS/web-platform vocabulary is now the working language,
so that "the WindowWatch unsubscribes on close" reads as a *kernel invariant*,
not a coding suggestion. Canonical terms: **kernel** = the `system/*`
substrate extensions; **the two heaps** = WASM linear memory ‖ JS GC heap;
**the frame contract** = the rAF loop; **the wire** = structured clone /
transferables; **arm** = Direct vs Worker SDK; **posture / deployment
profile** = the shipped access-control shape; **capability / namespace /
probe point** as in the canon.

**Decision discipline.** When a structural choice presents, the question is
**"what's right?", not "what's cheaper-but-compromised?"** — where *right*
means consistent with the layer's contract. **A choice that crosses a layer
boundary without a named interface is wrong even if it is locally cheaper. A
foundation fix that repairs a layer boundary is worth weeks; a feature that
papers over one is not worth a session.** (This is the user's "move slow to
move fast.")

**Doc discipline.** This charter is the lens; the MODEL doc is ground truth
for substrate behavior; the HARDENING-DAG is the navigation surface. When a
new doc lands it cites which layer it addresses and which disciplines it
engages.

**What stays as it is — the reframe is NOT a refactor license.** These are
already correct in the OS-discipline sense; the reframe names *why*, the
what/where stay (`[[feedback_reuse_before_abstraction]]`):

- **Entity-backed window state** — the tree IS the data model; window structs
  hold only `window_id` + `peer_id`. *The Plan-9 "everything is in the
  namespace" discipline.* Keep.
- **Subscription-driven reactivity (WindowWatch), no hashing** — render is a
  pure function of subscribed state. *The Elm/React reconciliation discipline.*
  Keep; do not reintroduce `compute_legacy_hash`.
- **`DomCtx` closure management** — the two-heap discipline already encoded as
  helpers. Keep.
- **The multi-SDK router / per-peer arm** — mixed Direct+Worker is *normal*,
  not exotic; the router is the right model. Keep; harden the arm-split footgun
  (D15), don't remove the router.
- **App-tier `WriterHandle` writers** — clonable, no per-writer arm branching
  (`[[feedback_app_tier_writers_both_arms]]`). Keep.
- **`app_paths` namespace ownership + the L0/L1 boundary visibility** — app
  conventions stay in the app, never in `entity-sdk`. Keep.
- **The transport stack** (`MultiConnector` / `xworker` / `ws` / `memory`) —
  the 1:N dispatch+lifecycle work is sound. Keep.

**What we are explicitly NOT deciding now** (prevents scope creep): WebRTC and
peer discovery (unbuilt — *correctly* absent, not a gap); the *default*
deployment profile for the lead release (the profiles — strict-site /
tutorial / full — ARE the E1 config mechanism, not a v1 architecture fork:
the system peer always exists; profiles differ in what's exposed and whether
the overlay is forced);
Site Mode P2 overlay (deferred until stable ground); wholesale L5-signal →
kernel-subscription migration (in-process coordination stays as-is; only
tree-derived reactivity is already migrated).

---

## 7. Adoption → enforcement → audit → maintenance

**Adoption.** Disciplines are promoted on bug-evidence (D12–D16 each cite a
shipped incident). This charter is ratified in the "standards & disciplines"
step (HARDENING-DAG node A1); until then it is *proposed*. New disciplines
land as **Pending** first.

**Enforcement (per-diff / per-PR).** The nine review questions run on every
change. Forcing functions are explicit per discipline (D12 grep, D13 e2e
`count_panics`, D15 caller grep, AP9 PR-time call-site review). Gate tests
encode disciplines as tests (a test that asserts a cleanup primitive is wired
*from production teardown*, not just works in isolation; cross-reload canaries;
a paired Direct/Worker arm harness — handoff §5).

**Audit.** Periodic whole-system, spec-grounded passes
(`[[feedback_architectural_review_altitude]]`), distinct from build sessions,
producing dated docs. Each opens with a D11 inventory boundary and prefers
**real-store / real-mode evidence first** (D10). The drift-audit shape: one
table row per check (`# | Check | PASS/PARTIAL/DRIFT/DEFERRED | Notes`), each
PASS *demonstrated* (quoted code or empty-grep), closed with a tally + an
open-thread tracker where **no finding is an orphan** (fixed / filed-upstream /
deferred-with-named-trigger / promoted-to-roadmap).

**Maintenance.** When a node lands → update the DAG + state; when a charter
discipline is added → update this doc + the AGENTS.md inline; when a new
gotcha surfaces → update the MODEL doc (single source — never fork an
explanation into a recipe). The structural shape (sandwich, disciplines,
review questions, anti-patterns) holds across refreshes.

**Session priming** (the "READ FIRST every session" ritual): AGENTS.md (always
loaded) → this charter (the anchor) → the MODEL doc (substrate ground truth) →
the latest handoff (carries the per-session discipline scorecard) → the
HARDENING-DAG (pick a route). The PARITY-MATRIX re-read at session start stays
(`[[project_parity_matrix]]`).

---

## 8. Bottom line

We inherit eleven disciplines because we are an entity-OS application, and we
earn five more because we are a **browser/WASM** application — and all five of
those were paid for in real bugs already in this tree. The charter's job is to
make those bugs un-shippable a second time: name the substrate, name the
contract each layer owes, turn each into a question we ask every diff. The
flagship gets built on this or it gets built on a pile.

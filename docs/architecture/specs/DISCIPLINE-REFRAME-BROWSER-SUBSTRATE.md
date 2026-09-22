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
*Amended 2026-08-18:* **"surface" includes the OPERATOR surface**, not only the
wire. A usage string, a `--flag` spelling, a refusal message, a make-verb name and
a default output path each state a contract, and each can drift out of agreement
with the code behind it — three of one audit's ten findings were exactly that, and
one of them meant *following the printed help could not succeed*. Its failure mode
here is **AP25**. Corollary earned in the same pass: a *stale doc comment* is
surface drift too — twice we found a **refuted** finding still asserted in a module
whose sibling already carried the correction.

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
holds keypairs only). The operator's acceptance test is the **cold return**:
leave, come back three weeks later, and find the work still
there. *How:* for every store we touch, document
durability + fallback + the cold-return story (the MODEL doc §persistence
pass); never gate work on a false "WASM has no filesystem" claim — OPFS/
IndexedDB *are* filesystems (`[[feedback_wasm_has_filesystem]]`); persistence-
sensitive code is tested in **each** WebView runtime (D10).

**Pending:** D17 (Application Knowledge — the model→output→renderer/T3
discipline as a first-class rule once we re-confirm where it pays off).
D18 (**candidate** — *One owner of truth per runtime resource; surfaces project
it, never mirror it.* A runtime resource's authoritative state — connection
liveness = the pool / the kernel `system/peer/status` entity — has exactly one
home; UI and app state *derive* from it, never keep a parallel event-sourced copy
that can disagree. Generalizes "state lives in the tree, not parallel structures"
+ D9-router from persisted to **runtime** resources. First incident:
`AUDIT-CONNECT-PEER-FILETRANSFER-2026-07-14`; ratify on a 2nd, different-shape
incident).
D22 (**candidate** — *Frame-loop panic resilience is per-loop, not just the rAF
loop.* D13/AP3 were written for `main.rs`'s single rAF loop; the `app_host` tick
clock is a **second** long-lived rendering loop — a `spawn_local` future — whose
panic (a debug-build overflow in the synchronous evaluator) unwound the task and
froze the board with no marker, the AP3 shape the discipline's letter didn't
name. Rule: *every* long-lived driving loop (rAF, tick, any repeated-render
`spawn_local`) owns a `catch_unwind` + visible-fault + recover/stop contract, not
only the one in `main.rs`. First incident:
`AUDIT-L5-COMPUTE-HOST-FOUNDATION-2026-08-01` #1 — fixed via
`program_host::host::guarded`; ratify on a 2nd loop repeating the shape).
D23 — **ratified 2026-08-27**, on the terms it set for itself; see the entry below.

**D19 — Every user-facing string goes through `t(key)`.** The string twin of
"theme via tokens" (raw English → a message key → the catalog, just as raw hex →
`var(--token)`). *Source:* `DESIGN-I18N-L10N.md` §9. *Why:* a hardcoded literal
is un-translatable and invisible to the locale switch — the exact shape colors
had before the token layer. *How / enforcement:* `crate::i18n::t()` is the one
string surface; `tools/i18n-lint.sh` (baseline-ratcheted, in `make lint`) gates
new raw literals in **anchored UI-emitting positions** (component label/title
args, `set_text_content`) — opt-in-anchored, not opt-out (most `format!`s are
URIs/CSS/diagnostics, not prose). **Gate is LIVE as of i18n P1**; the invariant
is *realized* incrementally as the P4 extraction ratchets the baseline to 0 —
so this stays **Pending→active** until the surface is fully migrated.

**D20 — Layout uses logical properties, not physical `left`/`right`.** The
direction twin of D19. *Source:* `DESIGN-I18N-L10N.md` §3.3, §9. *Why:* `dir`
has exactly two values (`ltr`/`rtl`, the non-string primitive — twin of a
theme's `scheme`); physical CSS (`margin-left`, `text-align:left`, `float`)
silently breaks RTL, and the app renders into a shadow root where `dir` must sit
on the host (finding 2). *How / enforcement:* `margin-inline-start` / `-end`,
`padding-inline-*`, `border-inline-*`, `text-align:start`/`end`; `dir` driven
onto the shadow host + `<html>` by `i18n::install_lang_dir`. **ACTIVE as of
i18n P3** — the atom layer + all 47 physical-direction sites are swept to
logical, and `tools/i18n-lint.sh`'s `phys` metric (in `make lint`) holds every
file at 0 (a new physical prop fails the gate). *Residual, NOT yet gated:* bare
`left:`/`right:` absolute-position insets are ambiguous (symmetric
`left:8px;right:8px` is dir-neutral) — the pseudo-locale (`en-XA`) visual pass
catches the directional ones (e2e asserts the shadow tree computes
`direction:rtl` under it). A full visual RTL sweep across every window is the
remaining manual check (`make tauri-run` with `en-XA`).

**D21 — L5 app compute runs behind the iframe boundary, in its own ephemeral
peer — never on the system peer.** *Source:*
`EXPLORATION-L5-APP-HOSTING-UNIFICATION` (P1/P2),
`REVIEW-L5-APP-HOSTING-BROWSER-2026-07-23`. *Why:* an "app" is a choice of
payload × isolation × contract; a WASM-entity-peer payload must be *isolated*
(P2) so a mount can't reach the system peer's tree or keys. The compute POC's
generic host first ran in the primary peer (a named scope gap); the L5 path
relocates that *same validated host* into a sandboxed inner peer, and the host
stays **blind to the payload** — it boots an app, the app emits state, the host
persists it (P1). *How / enforcement:* the payload boots via
`?app-host=<program>` (`app_host::run` — a lean ephemeral `Peers::new_direct()`,
no roster, no durable storage, branched in `main::start` before any
window-manager), hosted through the entity-apps ③α iframe (`dom::games`
`AppDelivery::Src`); e2e Phase 2h.2c asserts Life advances *inside the iframe-peer*
and the outer host persists its evolving state. **The sandbox is trust-tiered
(`dom::games::render_player`):** a **third-party JS app bundle** stays
`sandbox="allow-scripts"` (opaque origin — no reach into our origin/storage); an
**L5 app** is OUR own stripped browser-rust and gets `allow-scripts
allow-same-origin`, because the trusted payload must load its own multi-MB wasm and
an opaque origin fights that on **both** substrates — the browser CORS-gates the
`Origin: null` fetch, and Tauri's `default-src 'self'` never matches an opaque
origin (so the wasm glue is CSP-refused). Same-origin is safe *here*: the payload is
our code and its inner peer is memory-only (opens no IndexedDB). When L5 hosts an
**untrusted** app, this returns to opaque origin behind the sub-peer capability
model (`PROPOSAL-SUB-PEER-ISOLATION-MODEL`, DRAFT) — and *then* the opaque-origin
facts re-apply (already smoked, so they are on record): a `src`-served bundle carries
`Origin: null` and needs `Access-Control-Allow-Origin` on the browser dev/CDN server,
and under Tauri needs a custom ACAO-adding asset protocol **plus** the serving origin
in `script-src` (Tauri's `security.headers` refuses `Access-Control-Allow-Origin`, and
`'self'` never matches an opaque origin). **Two facts that stay true regardless:**
(1) `srcdoc` cannot carry a multi-MB wasm — L5 apps load by **`src`** (G1); (2) the
inner peer MUST be ephemeral — its state round-trips to the host (P1), so persistence
lives with the host, not behind the boundary. **Input is captured IN the iframe, never
across ③α:** the L5 payload is a focusable document running its own inner peer, so a
shape-bound keyboard driver (`app_host::input`) captures keydown/keyup on the payload
window and writes the input-port entity straight to the inner peer
(`host::input_future`) — the host still sees only `state` emissions and stays blind (P1).
The driver is the input mirror of the display driver — **program-blind, shape-bound**: the
entity's field name comes from the program's SEED (`shapes::input_field_name`), and the
value mapping is per shape (`direction`: arrows→`DIR_*`; `key-set`: a held-key bitmask,
its key→bit table composed from the program's OWN `scene.keymap` bit↔action map — never a
hardcoded binding). Display is likewise multi-shape: `display-list` (inline **SVG**
`<polygon>`s in a world-sized `viewBox` — DOM-native vector, not a canvas path; `scene.wrap`
seam-tiling is a noted follow-up) and `text` (a `<pre>` grid — kept in the vocabulary/driver
set but with **no current program exemplar**). Display presentation is DECLARED, not guessed,
the same lesson as the input roles (`RESPONSE-DISPLAY-RENDERING-AND-TEXT-REBIND`): a
`display-list` port declares **`scene.render`** ∈ `fill|stroke` (default `stroke`, so vector
games are unchanged) and reserves **`kind 0` as background the host MUST NOT draw**
(`build_display_list_svg` reads the render intent → filled coloured cells vs coloured
wireframe, and skips kind 0). **ACTIVE (browser + Tauri), three programs:** Life
(`display-list` `fill`, no input), Snake (`display-list` `fill` + `direction`) and Asteroids
(`display-list` `stroke` + `key-set`) — all pure-builtin (declare no `imports`, so they run on
wasm; the `compute/apply` stub gates only import-bearing programs). **Life/Snake were rebound
`text` → `display-list`** — a `<pre>` is a terminal in a GUI host, so the fix was the *shape*
(a filled grid of cell quads, Snake's head its own kind), not a glyph pass; the projections
are *dense* (a quad per cell, empties as kind 0), which is exactly why skipping kind 0 is a
required part of the contract, not an optimisation. e2e Phases 2h.2c/2h.2d/2h.2e assert each
advances behind the boundary (2h.2c also asserts Life paints a *filled* grid with kind-0
skipped), and 2h.2d/2h.2e deliver a real keydown into the same-origin sandbox and read the
payload's `data-app-host-input` D13 surface back (the boundary-crossing capture path the
native oracle test cannot exercise).
**The clock loop shares the parent's main thread.** A same-origin L5 iframe runs on the
*same* main thread as the outer app, and the compute evaluator is **synchronous** — so a tick
that overruns its budget blocks the outer UI (paint + input), and a build with an unoptimized
evaluator makes it visible (Asteroids felt sluggish until `entity-compute` was opt-level'd in
the dev profile, matching the crypto crates). The tick loop therefore (a) schedules by
*rate*, not by sleeping a full interval on top of the work, and (b) always yields a fixed
`MIN_YIELD_MS` floor so a heavy/slow tick can never starve the shared thread. Per-tick work is
surfaced as `data-app-host-tick-ms` (D13). The structural fix for heavier programs is a
Worker-hosted inner peer (the app's own Worker arm), still parallel.
**The named scope gap is now CLOSED — there is one honest "run a program" path.** The
Programs window (`views::programs`) was the last surface still mounting the generic host on
the *primary/system* peer (`text`-only, the original POC). It is now a **launcher**: it
lists the built-in `EMBEDDED_PROGRAMS` and, on select, runs the chosen program behind the
L5 boundary via the *same* `dom::games::render_player` + `AppDelivery::Src(?app-host=<key>)`
delivery — no compute on the system peer, and all three programs (not just `text`-shaped
Life) are reachable in production, not only under the e2e-only `demo-apps` fixture. The
window is **kept, not retired** — it is the "entity native programs" top-level surface where
user / entity-native programs running in local peers will later live — but its run action
never touches the system peer. Admission is enforced at the boundary: `app_host::run_program`
renders a **visible** refusal into the payload for a program binding a shape the host doesn't
drive (D13 — no blank iframe). e2e Phase 2h.3 now asserts the redirect (tile click →
sandboxed `app-host=<key>` iframe, no Install/Start/tick surface, Back → grid); the program
*running* behind the boundary stays covered by 2h.2c/d/e. The system-peer mount machinery
(`Mount`/`MountStatus`/install/start/tick loop) and the dead card renderer are gone; only the
shape drivers (`dom::programs::{text_driver,display_list_driver}`) survive, shared with the
app-host.
**Input is MULTI-SOURCE now — one target, many sources.** The write side of the `(role, shape)`
input ABI is a source-/boundary-agnostic `program_host::input::InputTarget` (owns a port's
encode context + its per-shape live state — the held-key mask for `key-set`), driven by
modality-neutral verbs (`set_direction`, `press`/`release`) and delivering via a
context-injected closure. A **source** translates its events into those verbs; the target stays
program-blind (action↔bit is the program's `scene.keymap`). Two sources ship today, both behind
the L5 boundary and both sharing ONE `Rc<InputTarget>` per port (so a keyboard key and an
on-screen button feed one mask, never two racing copies): the **keyboard** source
(`app_host::input` — keydown/keyup on the payload window) and the **on-screen pointer** source
(`app_host::onscreen` — a D-pad for `direction`, a button per program-declared action for
`key-set`, program-blind and shape-bound like the drivers). The on-screen pad is a
**corner-anchored virtual-gamepad HUD** (`position:fixed`, `vmin`-clamped touch targets with a
≥44px floor, movement/actions in opposite bottom corners, a ⇄ handedness swap). Its **default
visibility follows the device** — shown on touch, hidden on a precise-pointer+hover desktop
(`@media (hover:hover) and (pointer:fine)` on `data-mode="auto"`) — but an **always-present 🎮
chip** overrides either way, so a wrong device guess costs one tap (the reason a media query is
acceptable here where a whole-feature device *sniff* would not be). The earlier "show on every
surface" default was a workaround for the headless-`pointer:none` e2e trap (a green-≠-works F6
risk); the e2e now **force-shows via the chip** instead of assuming a default, so the real
default can follow the device without the test papering over it. **Stuck-key guard**
(`InputTarget::release_all` on window `blur` / document `visibilitychange`, the design §200 guard)
clears the held mask when focus leaves so a held key can't latch forever. e2e 2h.2d/2h.2e drive
BOTH sources into the same target across ③α (keyboard `right`=2 **+** on-screen `fire`=8 → one
shared mask `keys:10`) and assert the blur guard clears it (`keys:0`). The host now presents ONE
**standard controller**, not app-shaped chrome: a `key-set` port's manifest-declared control ROLES
(`program_host::controls`, the Rust mirror of workbench-go `ParseKeymap`) split into *directional
axes* (rendered on the one d-pad, momentary press/release — simultaneous presses OK, so Asteroids'
rotate+thrust work) and *discrete actions* (glyphed buttons, label/glyph from the manifest). So
Asteroids, re-declared, is a d-pad (left/right/thrust) + one 🔥 Fire button — the same controller as
Snake's `direction` d-pad, not four bespoke buttons (e2e 2h.2e asserts axes-on-d-pad + glyphed
action). The last host-owned guess `KEY_ACTIONS` (physical key → *semantic action*) is **retired**
for a program-blind **keyboard-position convention** (arrows/WASD → axis positions, a fixed key row
→ actions in bit order — the host names no app control). This was a cross-implementer contract
(`PROPOSAL-GENERIC-HOST-INPUT-DEVICE-MODEL.md`), **accepted** by workbench-go
(`RESPONSE-GENERIC-HOST-INPUT-DEVICE-MODEL-2026-07-24.md`: role hints + Asteroids re-declaration
landed, and the browser regenerated its bundled manifests from that). Three items stay open for
**arch** to ratify into the shape spec (RESPONSE §5): the standard action vocabulary + default
glyphs, the keyboard-position default (so every host agrees — the browser's is provisional), and
the action-button overflow policy.
The Life/Snake/Asteroids `demo-apps` tokens were dropped (they duplicated the production
`EMBEDDED_PROGRAMS`); the Programs launcher is the single "run a program" surface, and e2e
2h.2c/d/e launch through it.
**Program chrome splits program-owned STATE from generic host CONTROLS** (the operator's
"game modes are broken — every restart I have to leave the app; need a menu/reset and a
score"; RESPONSE-PROGRAM-CHROME-STATUS-AND-RESET). The diagnosis was that nothing was broken —
`Host.Restart` (reseed-to-state₀) always worked; the program view just had no chrome, so the
only reseed was leave-and-re-mount, and the program's score never reached a renderer. The fix
is one clean split, and it is the SAME "declare it, don't infer it" lesson as the input roles
and the render intent:
- **Score/state is PROGRAM-OWNED.** A program that wants a readout declares a SECOND output
  port, `status` (shape `text`, formatted in its OWN compute projection — `LEN 003 ▶` /
  `SCORE 00000 ▶` / `POP 0042 ▶`); the host relays it blind via the same `text_driver`, exactly
  as it relays the display board. The host never learns what a "score" is; because the bytes are
  formatted in the tree, every renderer (browser, Avalonia) shows byte-identical output —
  consistency by construction. This also gives the `text` shape its real exemplar back (a status
  line, not a game board) after Life/Snake moved their DISPLAY to display-list. The status port
  shares the `display` role with the board, so `descriptor::{display_port,status_port}` tell them
  apart by NAME, not order. Every program's oracle now carries a per-tick `status` hash, so the
  cross-impl gate verifies the projection tick-for-tick (`oracle_tests`).
- **Reset (↻) and pause (⏸) are GENERIC host controls**, not program inputs: reseed-to-state₀
  and clock-gating need zero program knowledge, so `app_host` presents the same two for EVERY
  program (even input-less Life). The tick loop owns the peer, so the buttons only set shared
  flags (`paused`/`reset`); the loop performs the reseed (re-rendering AND re-emitting state₀ so
  the host observes the reset across ③α) and gates the clock. A reset is now a button, not a
  re-mount.
The host stays generic: it iterates the output ports and captions the one named `status`; it
never computes a score, and `run_program` mounts the controls before any input port. e2e 2h.2c
proves all three (status caption renders `POP … ▶`; pause freezes `data-app-state-seq`; reset
while paused bumps the seq past the frozen baseline — nothing else can advance a paused sim).
**Meta-chrome sits AROUND the board, not over it** (the operator's "un-crowd the layout" —
settings floating over the play area is noise). Only the **thumb pad** overlays the board (the
mobile gameplay convention). The *settings-y* chrome — the reset/pause `.ah-hostbar` **and** the
🎮/⇄ input chips — moved out of `position:fixed` corners into ONE slim normal-flow bar
(`.ah-chrome`, `space-between`: host controls at the leading end, input chips at the trailing
end) ABOVE the board. The chrome bar and the board (`[data-app-host-display]`) share
`max-width:420px; margin:0 auto`, so they align as one centred column. The 🎮 chip still drives
the pad by a held `Element` reference, so relocating it out of the pad's subtree changes nothing
functional — the e2e's attribute selectors (`[data-controls-toggle]`, `[data-host-reset/pause]`)
are unaffected. `onscreen::build` now returns the pad and chips as two separately-mountable
elements (`OnscreenControls`) rather than one combined wrapper.
**Owed upstream (workbench-go/arch):** the same chrome in the Avalonia frontend (the bridge
already emits every port, so the status port reaches it for free) — the browser half is the
consumer landed here.
**Foundation audit (`AUDIT-L5-COMPUTE-HOST-FOUNDATION-2026-08-01`) — the ratchet the debug-panel
v2 rework skipped, run retroactively.** The surface was found **largely sound** (arm,
heap, namespace, kernel-reuse all clean) with drift in **failure observability + i18n reach**: the
tick loop wasn't panic-resilient (→ **D22 candidate**, `guarded`); the s-expr renderer had zero
tests (→ extracted to native `program_host::sexpr`, 14 tests); the payload's failure/refusal prose
was hardcoded English and dropped the host locale (→ `t()` + `i18n::apply(boot_choice())`, and
`src/app_host/` is now inside the i18n/UI gate globs so this can't recur silently); a
`MomentaryGuard` `Rc` self-cycle (→ `Weak` + `Drop`). **The one durable substrate fact:** the e2e
dist is a **dev/abort** build, so a REAL tick panic aborts the module there — the panic→`Err`
containment is gated by a **native** `guarded` test, the visible-fault **surface** by an e2e
query-param seam (`&app-host-fault-tick=N`); do not "upgrade" the e2e to a real panic.

**D23 — No unbounded network await on the boot path.** Between page load and the
frame loop running, every network operation carries an explicit deadline, and
exceeding it is a **state the boot proceeds from**, never a stall; a cached or
durable fallback that exists must be reachable within that deadline.

*Distinct from D13/D22*, which are about a loop that **panics**. This is a boot
that never **arrives** — and the observable is worse, because it is identical to
a hang with no marker at all: the frozen-frame watchdog installs *after*
`boot_load` returns (`main.rs`), so a boot that never returns never gets one.
No banner, no message, no exit.

*The recovery console counts, and it counts hardest.* **2026-08-27:** `tools/net-lint.sh`
originally scanned Rust and `assets/sw.js` only, leaving `index.html` — which carries the
service-worker registration *and* the L1 System Recovery BIOS — uncovered. That scope was
drawn around where the bug had been found rather than around where the rule applies. The
BIOS is the strictest case of D23, not an exception to it: it is what a user reaches
*because* something already hung, and an unbounded read there replaces a broken app with a
broken diagnostic, with no third tier beneath it. `index.html` is counted from that date, and
the behavioural half is `the_recovery_console_survives_a_blackholed_origin` — the panel must
reach a *reported* "could not reach the origin", never a spinner.

*Why it is not a rule about the word `fetch`.* **D23 bounds an await that is
blocking a defined alternative outcome.** A deadline is an improvement only when
there is something else to do on expiry — build-time defaults (D16), a cached
shell. Where the only outcomes are "the bytes" or "nothing" — a hashed asset
absent from the cache, where the URL *is* the version — a deadline converts a
slow first download of the ~30 MB bundle into a hard failure on exactly the
connections least able to afford it. Those sites stay unbounded **and say so**;
`tools/net-lint-baseline.txt` carries them by name rather than exempting their
files. Getting this boundary wrong in the enthusiastic direction is the likeliest
way to misapply this discipline.

*The distinction the whole thing turns on, because it is what made the symptom
disbelievable:* a network that **rejects** — interface down, DNS failure,
connection refused — rejects the promise promptly, every `.catch` and `.ok()?`
is reached, and boot continues. That case always worked, which is why the freeze
was intermittent. A network that **accepts and never answers** — captive portal,
half-open socket, a foreign LAN blackholing an address that used to work, an
overloaded CDN edge — never rejects anything at all.

*Source:* four same-shape instances. `assets/sw.js` `networkFirst` awaiting
`fetch` with no deadline while the cached shell sits unreachable in the `.catch`
(`grep -c setTimeout assets/sw.js` → **0**); `deployment_config::fetch()` on the
**cold** boot path, shipped and live on the build both production domains were
serving, plus R1's widening of it to warm boots
(`AUDIT-REKEY-RECONCILE-2026-08-27` F1); the worker `Ready` wait under build-key
skew (`REVIEW-2026-08-25` §3.3). Two languages, three subsystems, no shared code.
Prior art the omission is measured against: Workbox ships
`networkTimeoutSeconds: 3` on navigations by default, for this exact symptom
(AP28).

*Ratified on evidence, not on the count.* It was filed as a candidate precisely
because all instances were **code-read and none reproduced**, and it named its
own condition: *"ratify when G1 — an origin that accepts and never answers —
reproduces the blank page and the fix closes it."* That run now exists.
`tools/e2e/blackhole-serve.py` serves `dist/` and accepts-without-answering a
nominated path — the one thing `python3 -m http.server` cannot do, and the reason
this failure class had never been expressible in the harness. Observed **red on
the unfixed tree first**, with the failure being the stated mechanism: the
captured console stops dead at `app.rs:1740`, the line immediately before the
fetch, and never reaches `Frame loop started`. Both gates green after the fix.

*How / enforcement — all three, as owed:*
1. **`src/net.rs`** — `net::fetch_text_bounded()`, the one bounded read. It
   covers **headers and body under a single deadline**: bounding only the header
   phase leaves the identical hazard one step later, since an origin may answer
   `200` and then never send a body. It therefore does not expose a `Response`.
2. **`tools/net-lint.sh`** (in `make lint`, baseline-ratcheted) — raw
   `fetch_with_str` / `fetch_with_request` outside the chokepoint, and bare
   `fetch(` in `sw.js` outside `fetchWithDeadline`. Verified by mutation: it
   fires and names the file.
3. **G1** — `boot_survives_a_blackholed_deployment_config` and
   `a_cached_shell_survives_a_blackholed_origin` in `tests/e2e_worker.rs`, both
   anti-vacuity guarded on the **server's own log** (the request must have
   arrived and been stalled), because a boot that never asked would otherwise
   satisfy every assertion for the wrong reason.

Neither static half subsumes the other: the gate proves a deadline is *honoured*
but only for the fetches that exist today on the paths it exercises; the lint
proves no *new* raw fetch has appeared but cannot tell whether a deadline is
respected.

*One deadline covers BOTH stall shapes, and that is measured rather than
derived.* There are two ways an origin can stall: send nothing at all, or send
complete headers and then no body. The second looked like a hazard the fix would
miss, because `fetch` is specified to resolve on **headers** — so a deadline
clearing its timer at that moment would be disarmed exactly when the body read
began, and the hang would move to `await cache.put('/', resp.clone())`. It does
not: `a_cached_shell_survives_an_origin_that_stalls_the_body` is green in ~5 s at
`NETWORK_DEADLINE_MS = 3000` and **red at 300000**, so the deadline is
demonstrably what saves it. That gate is kept although it found nothing to fix,
because the property is **engine behaviour rather than ours** and can therefore
change without any diff of ours touching it.

*What this does NOT close.* The `sw.js` deadline covers a **cached** shell. A
first-ever visit to a black-holing origin has nothing to fall back to, and no
deadline creates one — that remains E1 (reload retries), and it is the residual
the boot-slot work addresses, not this discipline.

**D24 — Any durable copy of someone else's bytes is a cache, and needs a
currency trigger. `if absent` is not one.** The durable entity tree is the single
source of truth *for state we own*. The moment a foreign artifact is written into
it under `/{me}/{foreign}/…` it is a **cache** of a remote mutable artifact, and
it needs a freshness model like every other cache layer here. A consumer may
guard on *"have I asked this session"*; it may not guard on *"do I already hold
a copy"*.

*Why this is a missing abstraction and not a reminder.* Every cache we designed
**as** a cache has a freshness model — `sw.js` is network-first for the mutable
shell, the browser HTTP cache gets `Freshness::Mutable` → `no-store`, the registry
has a TTL. The one that became a cache **by accident** has none, and it sits on
top of the ones that are correct: a presence check in the store short-circuits
all three, **because no request is issued at all**. `if cached.is_none() {
fetch() }` is the natural thing to write, it is correct for immutable content,
and nothing about the store signals that this path holds someone else's mutable
artifact. That is why it recurred per app, per site, per feature.

*We did not have to invent change detection; we were bypassing it.*
`fetch_entity_two_hop` already splits a **pointer** at a stable path
(`Freshness::Mutable`, `no-store`, 58 bytes, changes iff the entity changed) from
a **body** addressed by its hash. The pointer *is* the version and the answer is
exact — not a TTL, not a heuristic. And the local half of the comparison is
already in the tree: an `Entity` carries its canonical `content_hash`, the same
`Hash::compute(type, data)` the pointer holds. **The pin was never missing.**
`CacheProvenance::pinned_root_hash` — added to hand-roll this answer at a second
layer, and read by nothing that decides — is therefore not merely unwired, it is
a duplicate of a fact the substrate already stores. Two correct implementations
of change detection and a consumer layer that called neither.

*Source:* four incidents in two shapes, which is what promoted **AP30** from
anti-pattern to discipline. The **record** shape (`peer_supersession`, 2026-08-27:
a durable record of a remote assertion with no way to re-ask) and the **cache**
shape (2026-08-28, found on live production by devops): the **app bundle**, fetched
only when absent, so every returning profile ran the app code it first downloaded,
**silently** — `boot_load: complete`, frame loop armed, every fetch `ok`,
week-old app on screen; `precache_origin_sites` **skipping every manifest it
already held** while its own sibling `warm_peer_sites` refetched unconditionally;
and the already-fixed **catalog**, whose repair landed on the file carrying
metadata and not the file carrying the app, in a diff that stared at both.

*Two corollaries, both of which are where the naive version goes wrong.*
**(a) Absence of evidence is never evidence** — an unreachable origin, an
unparseable answer or an expired D23 deadline must leave the held copy
**untouched**. A cache that drops what it cannot re-verify converts a brief
outage into a missing app, which is strictly worse than the staleness being
fixed. **(b) Unchanged must be free of side effects** — no durable write, no
dirty flip. A "refresh" that rewrites an identical record every boot is
accumulation (D9), and on this surface it would replace a running app's
`<iframe>` and restart it.

*How / enforcement — both halves, as owed:*
1. **`src/content_site/foreign_cache.rs`** — `ensure_current`, the one entry
   point, owning presence **and** currency. `Currency` is
   `Fetched | Unchanged | Unavailable` and deliberately **has no variant meaning
   "I already had one, so I did not look."** `HeldHash` can only be produced by
   `held_hash`/`held_set`, so a consumer cannot hand it a judgement of its own.
2. **`tools/foreign-cache-lint.sh`** (in `make lint`, baseline-ratcheted) —
   direct calls to the per-artifact fetchers outside the entry point. It lints
   the **module boundary**, not the defect's shape: the audit's proposed rule
   (a durable read of a foreign path used as a fetch guard) is the true one and
   an unreliable grep, and **AP29** is the entry for a gate that counts prose.
   Verified by mutation — it fires and names the file.
3. **`an_app_republished_under_a_stable_identity_reaches_a_returning_profile`**
   (`tests/e2e_worker.rs`) — publish, boot, **republish under the same
   identity**, boot the same profile again, assert the new bytes are in the
   player's `srcdoc`. It asserts the rendered marker, never that a fetch
   happened (AP31), and its negative half asserts the **old** marker is gone.

*Ratified on the gate, not on the count.* AP30 named its own condition — *"a
discipline with no enforcement point does not count; ratify D24 in the same change
that lands the gate"* — because none existed for the cache shape. **No gate
anywhere in this repo had ever visited an origin twice across a publish**, which
is exactly why devops found this in minutes on a real deploy and a green suite
never could. The gate now exists, was observed **red on the unfixed tree** with
the failure being the stated mechanism (the returning profile still rendering
`APP-MARKER-V1`), and green after the fix. Its fixture also reproduces the
production shape offline for the first time: across two publishes the catalog is
**byte-identical** (`146bfc3d…` → `146bfc3d…`) and only the bundle pointer moves
(`cfbfc227…` → `e48d98cb…`) — which is devops' edge measurement (139/139 objects
byte-identical, `active_version` unmoved) in a test.

*What this does NOT close.* The lint cannot catch a consumer that calls
`ensure_current` *conditionally* on holding a copy — the boundary stops the
fetch from being reachable, not the call from being guarded. That is what the
gate is for, and it covers one artifact (an app bundle) on one arm (Direct-IDB).
The **Worker arm** has executed none of it: there, the sync read answers from a
per-subscription mirror that fills asynchronously, so a Worker run can miss its
own cache, refetch, and go green for a reason unrelated to the fix. A Worker run
is a second, separately-labelled assertion — never the one quoted as proof.

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
5b. **What did this change make redundant, and did I delete it?** If it
   introduced an authoritative source for a fact something else already stored,
   the mirror goes in the same arc — or the remaining read is a dated,
   written-nowhere migration fallback with its removal condition recorded
   (D9, AP17). "We demote it later" is the tell; there is no later.
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
- **AP12 — Mirror-first architecture: reinventing an unbuilt kernel extension
  with app-tier mirrors.** `connections` + `connection_health` substituted
  hand-rolled, event-sourced mirrors for the *spec'd-but-unbuilt* connection
  owner (`EXTENSION-NETWORK` / `system/peer/status`) instead of consuming the
  kernel model or routing the gap upstream — so the app must later *unwind* the
  mirrors rather than converge. The tell: a subsystem grows across sessions while
  every diff passes the nine questions locally, because per-diff review never
  asks "does a core extension already own this?" [D1, D8,
  `AUDIT-CONNECT-PEER-FILETRANSFER-2026-07-14 §8b-§8d`]
- **AP13 — Compensate a stale mirror with a second mirror.** A derived copy
  drifts (the add-only `connections` registry, stale-forever), so a *second*
  derived copy (`connection_health`) is added and read "instead" — both drift the
  same direction; now two lie. The fix for a drifting projection is to reconcile
  to the source, never to add another projection. [D9]
- **AP14 — Auto-heal keyed to the wrong error class.** `execute_reauth` retries
  only on `Ok(status==403)`; the real failure is a transport `Err` ("closed
  connection"), so the recovery path compiles, tests green on the class it
  handles, and **silently never fires** on the class that actually happens.
  Recovery must key on the failure that occurs, proven by trace. [D7, D13]
- **AP15 — Arm-split: a capability installed on only one arm.** The second
  instance of AP4's family, one level up. The §6.5 WebRTC establisher was wired
  into the Worker arm only — not from a platform constraint (`with_live_establish`
  is arm-neutral and already present on Direct; `RTCPeerConnection` is main-thread
  on *both* arms) but because the Worker path was built first for S5. Result: a
  whole capability unreachable in the **shipped Direct/IDB** deployment, invisible
  because native + Worker-e2e were green (AP7). The tell: an arm-neutral seam that
  is *present* on both arms but *called* on only one. Fix: install on the shipped
  arm first; the A-series moved it to Direct and proved it (`make e2e-webrtc-chat`
  default mode). [D15, `[[project_peer_sdk_arm_model]]`,
  `HANDOFF-2026-08-06-direct-arm-webrtc-PROVEN`]
- **AP16 — Demoting a "hack" without testing the shipped arm with it off.** The
  chat delivery poll was assumed a worker-era crutch, to be demoted once the
  Direct arm had subscribe + WebRTC. Demoting it to a slow reconcile **regressed
  delivery asymmetrically** (B→A reactive-OK, A→B missed) — the fast poll was
  silently *retrying §6.5 establishment* (hundreds of offer deposits, not one) and
  *covering the direction subscribe misses over WebRTC*. A "redundant" mechanism
  is only redundant once the thing it silently backstops is proven to stand alone.
  Test the shipped arm with the mechanism removed **before** calling it a hack.
  [D10, D13, `HANDOFF-2026-08-06-direct-arm-webrtc-PROVEN`]
- **AP17 — Half-converged migration: introduce the authoritative source, keep the
  mirror.** The correct fix for AP12/AP13 is to consume the kernel model — but
  landing the kernel source *without retiring the app-tier copy in the same arc*
  leaves **more** duplication than before, not less, and with no rule about which
  copy wins. *Incident (2026-08-11 → 08-13):* publishing `system/peer/transport`
  routes closed the "app is the address book" inversion, and `connections.addr`
  stayed. For two days a remote peer's address lived in **three** durable places
  (`connections.addr`, `system/connection.address`, the route) across **two key
  spaces** (Base58 vs identity-hash hex), while the retirement sat on a
  "remainder" list and was deprioritized twice — each time for a defensible
  local reason. The tell is the phrase itself: *"and later we demote X"*. There
  is no later; the mirror is load-bearing until something stops reading it.
  **Rule: the arc that introduces an authoritative source is not done until the
  mirror's last reader is gone or the remaining read is a dated, written-nowhere
  migration fallback.** A read-only compatibility shim is acceptable *only* with
  its removal condition recorded at the same change (D16 cold-return is the
  usual reason one is needed). Its own forcing question, now question 5b below:
  **"what did this change make redundant, and did I delete it?"**
  [D1, D9, `MODEL-REMOTE-PEER-FACTS` §2]
- **AP18 — A render input with no dirty signal of its own.** The render reads an
  in-memory structure — a `thread_local!`/`static` registry, a derived cache —
  that can change *without* dirtying the surfaces that read it, so its
  invalidation is borrowed from some *other* subscriber's watch. Whenever the
  render wins that race the correction is never painted, and because the borrowed
  signal was consumed, **nothing ever repaints it**. The surface is permanently
  wrong, which reads as "flaky" but no amount of re-polling touches it.
  *Incident (2026-08-13):* the Settings theme dropdown is a pure read of
  `theme_tokens::USER_THEMES`; `register_user_theme` / `unregister_user_theme`
  mutate it while touching no `DirtyFlag`; `frame()` reconciled the registry
  *after* `dom.render`. A theme deleted from the tree stayed in the dropdown
  forever — 15s of polling with forced layout flushes changed nothing, which is
  the signature. **Rule: anything the render reads must be invalidatable.** Give
  the structure its own `DirtyFlag`, or reconcile it before the render and treat
  that order as load-bearing (and say so at the call site). Note the diagnostic
  trap this creates: `peer_registry.sync` sits under the *same* line order and is
  **fine**, because it writes to the tree and tree writes dirty their watchers —
  so **check the mechanism, not the position in the frame.**
  [D4, D13, `AUDIT-THEME-DELETE-STALE-DROPDOWN-2026-08-13` §8 F1]
- **AP19 — The optimistic update as test crutch.** A handler mutates local state
  *and* dispatches the durable write, and the test cannot tell which one
  satisfied it — so the test passes on the optimistic path while the reconcile
  path underneath is broken, and reports green over it.
  *Incident (2026-08-13):* e2e Phase 26.8 exercised delete-a-theme for a session
  while the projection was broken, because `delete_theme` unregisters
  synchronously *before* dispatching the remove; the reconcile it nominally
  tested was a no-op (`removed=[]` in the trace). **Rule: drive the change from
  the far side** — make it happen where only the reconcile can carry it (Phase
  26.9 deletes straight from the tree via the Shell's `rm`) — **and verify the
  gate red before the fix.** A gate never seen red is not a gate.
  [D10, `AUDIT-THEME-DELETE-STALE-DROPDOWN-2026-08-13` §10]
- **AP20 — Invalidation narrower than the read.** A cache is *read* across one
  scope and *invalidated* across a smaller one. Every read sees the whole shared
  surface; each change wakes only the one participant it was addressed to. A
  reader whose own event arrives before the others' observes a half-updated
  shared state and is never woken again — the events that finish the update
  belong to somebody else. Permanently stale, and **which** reader loses is
  decided by delivery order, so it presents as flake and is immune to polling.
  *Incident (2026-08-13):* `WorkerProxy::cache_get`/`cache_list` answer from the
  **union of every subscription's mirror**, while each `Change` poked only the
  addressed `sub_id`'s `notify_tx`. Four subscriptions held the themes prefix; a
  delete arrived as four events; the reader owning the second reconciled against
  a union the other two hadn't cleaned, and never reconciled again. **Rule: any
  shared read surface must be invalidated across the same scope it is read
  across.** The tell is a per-item/per-subscriber notification sitting next to an
  aggregate read — if you merge N sources on read, you must wake all N on write.
  [D4, D14, `AUDIT-THEME-DELETE-STALE-DROPDOWN-2026-08-13` §8 F3]
- **AP21 — Detecting change by diffing around your own pump.** A frame-pumped
  component reports "did anything change?" by snapshotting its state before and
  after calling its own `pump`. But a pump's job is to *start* asynchronous work;
  the results land **between** frames. So the two snapshots are equal precisely
  when something did change a moment earlier, the caller reports no change,
  nothing repaints, and the surface freezes on a stale state forever. The tell is
  a comparison whose two sides are taken microseconds apart around a call that
  spawns.
  *Incident (2026-08-13):* `PeerConnectionsModel::pump_meet` diffed
  `MeetSession::status()` around `session.pump()`. A meet whose dial had already
  **failed** kept rendering "Searching…" indefinitely — in a section written
  specifically to never leave a search unresolved. **Rule: the component owns a
  change flag set at every mutation site — including inside its spawned landings
  — and the surface consumes it (`take_changed`).** This is AP18 in a component
  that has no tree write to ride: same rule ("anything the render reads must be
  invalidatable"), different carrier. And note what it cost to find: three native
  tests and a native poll-until-true loop all passed over it, because a loop that
  re-reads until it sees the change cannot notice that nobody was *told*.
  [D4, D13, `STATUS-2026-08-13-naming-modes-meet-at-a-name`]

- **AP22 — A capability whose decision has no surface.** A feature is split into
  a *capability* (may we?) and a *decision* (do we?), which is right — until the
  capability becomes user-reachable and the decision does not. Then every build a
  user actually runs satisfies half the condition and silently does nothing. It
  passes every gate, because the harness sets the decision by URL or build knob,
  which is exactly what a user cannot do. The tell: a decision knob readable only
  from `option_env!` or a query param, guarding a capability that a *UI surface*
  can now supply.
  *Incident (2026-08-13/14):* the §6.5 establisher needed `ENTITY_WEBRTC_ENABLE_
  PRIMARY=1` **as well as** a signaling node. When the connector registry landed,
  the node became something a user selects in the window — but nothing ever
  granted the second half, so adding a connector and meeting at a name produced a
  peer-id that could never be connected to. Inert in every shipped build; green in
  `e2e-webrtc-chat`, which passes `?webrtc_enable=1`. **Rule: when a capability
  becomes user-supplied, the user's act IS the decision — collapse the axes and
  keep only the fail-closed half.** Ask of any two-part gate: *which surface
  performs each half, and can the same person reach both?*
  [D3, D13, `HANDOFF-2026-08-14-webrtc-install-decision-and-the-meet-gate`]

- **AP23 — Asserting a dispatch instead of an effect.** A UI harness performs an
  action (click, keystroke, synthetic event) and then asserts on the outcome
  after a fixed sleep — treating "I dispatched it" as "it happened." When the
  action silently does not apply, the harness reports the *application* as broken,
  and every subsequent theory is about the app. The tell: a test step whose only
  evidence that it acted is that the call returned.
  *Incident (2026-08-14):* the meet-then-chat harness clicked "+ Shell" and typed
  in the same instant. A spawn click only **queues** an action — the window is
  created on the next frame — so there was no input element and nothing was
  typed. It presented as a post-reload Shell freeze and cost most of a session:
  five app-side causes (frame panic, reload loop, write storm, dead frame loop,
  dropped action) were investigated and refuted before the harness was suspected.
  **Rule: confirm the action from the app's own output before proceeding, and
  retry until it does; never assert on the echo of your own input** (the Shell
  echoes the line it is given, so asserting on scrollback growth after a
  `connector` verb passes whether or not the verb ran). This is the e2e's
  `poll_json`-not-`sleep(fixed)` rule extended from *waiting* to *acting*.
  [D13, `HANDOFF-2026-08-14-webrtc-install-decision-and-the-meet-gate`]

- **AP24 — Trusting a rig to have the property it was built to model.** A harness
  is constructed to reproduce an environment (a NAT, an offline origin, a slow
  link, a storage-denied context), and from then on that property is *asserted by
  construction* rather than measured. When the rig is subtly wrong, every symptom
  it produces is read as an application defect — and the symptoms are real, so the
  investigation is well-evidenced and entirely misdirected. This is AP23 one layer
  out: AP23 is a harness that did not do what it claimed, AP24 is an environment
  that is not what it claimed. The tell: the rig's defining property appears in its
  *setup script* but in no assertion, and no probe smaller than the application
  ever exercises the path.
  *Incident (2026-08-14):* the two-NAT traversal rig was red for most of a session
  and two app-level blockers were reported upstream — `429 bucket_full` and
  `addIceCandidate: Unknown ufrag`. **Both were symptoms of a defect in the rig's
  own NAT.** The routers accepted *unsolicited inbound* UDP, which means a
  conntrack entry was holding the exact reply tuple each peer's outbound punch
  needed; the outbound then lost its advertised port and was remapped, so both
  sides sent from ports the other had never heard of and the punch could never
  converge (measured: A advertised `10.89.3.2:60449`, sent from `:32941`). A real
  NAT drops that packet and keeps no state. Both "app bugs" vanished when the punch
  started landing, and the green run logs zero negotiation failures. What found it,
  after an afternoon of theorising about window sizes and candidate counts, was a
  **thirty-line bare UDP hole punch** between the same two containers — one socket,
  STUN, punch — whose `NO-PACKETS` moved the investigation out of the application
  in a single step.
  **Rule: prove the environment's path with the smallest probe that can carry it,
  before attributing a failure to the application** — the probe must share the
  rig's topology but none of its code. **And a rig must *measure* the property it
  models, as a control that fails the run.** Ours now probes its own controls (no
  direct path; two DISTINCT external addresses; host reachable through each NAT),
  because a rig that silently degrades does not merely stop testing — it "proves"
  the opposite of the truth (here: that host candidates traverse NATs). Note the
  second-order trap that let the bad rig look good: the first mapping test varied
  only the destination **address**, both observers on port 3478, and so called a
  symmetric NAT endpoint-independent. **Vary every axis of the property you claim
  to be measuring.** Corollary kept: an unjustified constant is worse than none —
  the negotiation window was widened 15s → 45s on a guess, changed nothing, and was
  reverted rather than left in as a talisman.
  [D13, `HANDOFF-2026-08-14-nat-traversal-works-and-what-it-cost-to-learn`]

- **AP25 — The operator surface is a surface, and no gate reads it.** Usage
  strings, flag spellings, refusal messages, make-verb names and default output
  paths are as much a contract as a wire format — and they drift *faster*, because
  nothing in the suite constructs them. Every test reaches the feature through its
  Rust API, so the printed help can teach a form the parser rejects and the suite
  stays green forever. The tell: a surface a human types or reads, with no test
  that types or reads it.
  *Incidents (2026-08-18, two shapes, which is what ratified it):* **(a)** the
  `registry` help printed `--bind NAME=PEER_ID`; the parser matched only
  `--bind=`, so the flag was silently dropped and the refusal read *"at least one
  --bind is required"* — **to someone who had just passed one** — while also
  omitting the `@ORIGIN` arch D10 had made mandatory. Following the printed help
  could not succeed. **(b)** nothing validated that a binding's target *is* a
  peer-id, so a directory slug typed where the id belonged emitted a fully
  **signed** binding, reported as success, whose only symptom surfaced on a
  *consumer's* machine as "the named peer-id carries no public key". Adjacent:
  `make federation` defaulted its output inside the directory `trunk` wipes, and
  publishing a registry had no `make` verb at all on a podman-only host.
  **Rule: a refusal must be assertable as a value, not merely as an exit code.**
  The fix is not a better string — both the right refusal and the wrong refusal
  exited 1, which is exactly how (a) survived. Parsing moved into
  `parse_registry_args`, which returns `Result<_, String>`, and four tests assert
  on the **message**. Any new operator-facing refusal owes the same. Corollary:
  **a fixture that could not exist in production hides the check that would have
  caught it** — the registry fixtures said `"2PEERTARGET"`, itself not a peer-id
  in any form. And: four of that audit's ten findings were found by *running* the
  tooling rather than reading it.
  [D8, D10, `AUDIT-NAMING-AND-PUBLISHING-ARC-2026-08-18`]

- **AP26 — A self-heal scoped to the instance, gating out the class.** A repair
  written for one instance of a failure, guarded by a condition that only that
  instance satisfies, does not merely fail to cover the rest of the class — the
  guard **prevents** the broad path from ever running, so the class is *less*
  reachable than if no repair existed. The tell is a narrow, cheap precondition
  in front of an expensive-but-general recovery: `if <the exact symptom I saw>
  { <the general fix> }`. The general fix is right there; the `if` is what
  stops it.
  *Incident (2026-08-24 → 27, `ecdeos.org`):* a domain re-keyed onto a new
  publisher identity, and every returning visitor kept asking the retired one —
  newest WASM, 404s, an app that reports healthy. A warm-boot reconcile already
  existed for this class and its comment stated the general principle correctly
  (*"the site-origin REGISTRY is a routing FACT, not a preference … stranded
  with no way to self-heal"*). But it re-fetched **only when the home peer's
  origin was unregistered** — it repaired *the origin for the peer you already
  have*, and could never repair *the peer you have being wrong*. The first cold
  boot had registered that origin, so the guard was false and the re-fetch never
  ran.
  **Two things this cost, and they are the reason it is catalogued rather than
  just fixed.** First, *the guard made the bug invisible in the log*: the
  identical console is produced whether the reconcile ran and compared the wrong
  pair or never ran at all (`unwrap_or(false)` collapses `None` and
  `Some`-without-the-key onto one branch), so **two documents independently
  concluded the reconcile had fired and the fix was a one-line field
  comparison** — a fix that would have shipped looking correct while healing only
  the minority whose origin seed had failed. The discriminator was a single
  absent `warn!` line, and it took a live console to settle.
  Second, the repair had a second half nobody had named: the overlay's
  navigation state persists its own `peer`, so correcting the session config
  alone leaves the user parked on the retired publisher **while the config
  reports healthy** — the original defect wearing new clothes.
  **And the first attempt at that second half repeated the anti-pattern one
  level down**: it re-pointed the *overlay* and left a Site Browser **window**
  (how `entitychurchfoundation.org` deploys) still on the retired peer, filed as
  a "known gap". A fix scoped to the surface the author was looking at is the
  same mistake as a reconcile scoped to the symptom the author had seen. The
  operator's correction is the rule: **a peer being replaced is ONE fact about a
  peer, not N facts about surfaces** — so it is recorded once
  (`src/peer_supersession.rs`) and resolved at the single decode point every
  surface shares, which also covers surfaces that do not exist yet. A *sweep* of
  stored state was the obvious alternative and is unreachable: the recursive
  enumeration is arm-dependent and the async one drops directory entries at the
  worker boundary, so a sweep silently misses entries on one arm — AP26 a third
  time, in the repair for AP26.
  **Rule: write the reconcile at the width of the *class*, and when you
  supersede a narrow one, retire it rather than gain a sibling.** Two
  reconcilers with overlapping triggers is how the first came to hide the
  second. Corollary, earned twice here: **when two code paths produce a
  byte-identical observable, one of them must be made to say so** — the absent
  log line was the whole diagnosis.
  *Enforcement:* `rekeyed_domain_heals_on_next_boot` **and
  `…_window_surface`** (`tests/e2e_worker.rs`) run the identical scenario over
  BOTH deployment shapes — publish under one identity, cold-boot, re-key, retire
  the old tree, reload the same profile. Two surfaces because one surface is how
  this was got wrong. Each asserts **anti-vacuity** first (the boot was warm, the
  re-key was detected, a supersession was recorded), since every other assertion
  is also satisfied by a browser that merely cold-booted and never held the
  retired peer.
  *Corollary the proving cost, three times in one sitting:* **a selector that
  looks precise and is not, whose failure surfaces away from its cause.** A
  libtest substring filter also matched a longer fixture name (wrong surface
  booted, failure reported as a product bug); `--exact` then matched nothing
  because it compares the FULLY-QUALIFIED name, and **libtest exits 0 for "ran no
  tests"**, so the fixture became a silent no-op that passed its own status
  check; and publishing fixtures into the shared `dist/` broke the monolith's
  Phase 27 with *"publisher bound no signature"* — green when that phase ran
  alone. **Rule: a fixture asserts that it ran, and does not share a directory it
  did not create.**
  [D9, D16, `DESIGN-RESILIENCE-RECONCILIATION-AND-ENTITY-DOCTOR` §1.1a; arch's
  L14 — *a rule written at the width of the incident* — is the same shape seen
  from the spec side]

- **AP27 — Escalating analysis as a decision.** Presenting the settled consequences of the
  model as a menu for the operator. It reads as deference and functions as avoidance: it
  moves work that requires *reading the system* onto someone who is being asked to rule on
  it without one, and it launders "I did not finish the analysis" into "this needs your
  input". The tell is a decision list whose options can be closed by re-reading the
  requirements, the extensions, or the data model.
  *Incident (2026-08-27):* a six-item "decisions only the operator can make" list. Five
  collapsed on contact. Adoption granularity is **per entity** — it is in the store, it has
  a path, it has a hash, it changed — because that is what the model *is*, not a preference;
  the "atomic vs per-item" framing had invented a choice the system does not offer, and used
  the wrong noun for the one it does. The ownership line was **malformed**: there is no
  meaningful user peer yet, only our system peer and our publishing peers. DNS-shaped names
  needed a **rename**, because the only defect is a pattern implying enforcement we neither
  have nor want. Prerendered HTML is **not optional** if no-JS readers are supported.
  Retention depth was **unanswerable** as posed and needed the prior analysis first. The
  sixth was withdrawn as too poorly explained to be answered — our defect, not an open
  question.
  **Rule: if the analysis makes the design clear, it is not a decision — do the design.**
  Escalate when the answer genuinely turns on intent, cost the operator alone can weigh, or
  an outward-facing commitment. A real decision is recognisable; these were not.
  Corollary: **a "known gap" filed in a fix is usually this anti-pattern wearing a different
  hat** — see AP26, where the surface left unrepaired was booked as a gap rather than as the
  unfinished half of the work.
  [D9, `HANDOFF-2026-08-26-deployment-and-resilience-where-the-design-landed` §5]

- **AP28 — A hand-rolled standard mechanism, minus the safety option the reference
  implementation defaults to.** Re-implementing a well-trodden pattern from its *description*
  rather than from a canonical implementation, and thereby omitting the guard that the
  canonical version considers so essential it ships it on by default. The omission is
  invisible to review because the code matches the pattern's name and its happy path exactly;
  what is missing is the clause that only exists because the original authors hit the failure.
  *Incident (2026-08-27):* `assets/sw.js` implements network-first caching in ~50 well-reasoned
  lines with an extensive comment block — and no timeout of any kind
  (`grep -c setTimeout assets/sw.js` → **0**), so the cached shell one line below is reachable
  only when the network *rejects*, never when it accepts and stalls. Workbox — Google's
  reference implementation of the same strategy — exposes `networkTimeoutSeconds` and its
  `pageCache()` recipe sets it to 3 for navigations, with the documented rationale being
  verbatim the symptom we shipped: *"loading spinners spinning endlessly."*
  **Rule: when implementing a named pattern, read a canonical implementation's OPTIONS, not
  just its description — the options are where the field experience is recorded.** A pattern's
  defaults encode failures someone else already paid for; declining them is a decision that
  must be made deliberately and written down, not made by not knowing.
  Note the ladder position honestly: this is a **first** instance. D12
  (*read canonical sources*) is the discipline it is already an instance of; what is new is
  that "canonical source" includes the **API surface of a reference implementation**, not only
  specs and upstream docs.
  *Closed 2026-08-27:* `fetchWithDeadline` in `assets/sw.js`, at Workbox's own 3 s for
  navigations — the number borrowed rather than invented, which is the point of the entry.
  The mechanism is no longer code-read: `a_cached_shell_survives_a_blackholed_origin` was
  observed red on the unfixed worker (the reload never painted although the shell was
  cached) and green after.
  [D12, D23, `DESIGN-CODE-AXIS-RECOVERY-AND-BOOT-SLOTS` §1.1b, §2.1]

- **AP29 — A gate that counts prose.** A grep-based lint whose pattern matches the
  *documentation of* the thing it forbids as readily as the thing itself. It reports a
  violation in the file that has just been fixed — because the fix's comment names, by
  necessity, the raw call it replaced — and the cheapest way to make it green is to delete
  the explanation. **A gate that charges you for documenting its own rule teaches people to
  stop documenting it**, which costs more than the drift it was built to catch.
  *Incident (2026-08-27):* the first `tools/net-lint.sh` flagged
  `src/deployment_config.rs` for a `fetch_with_str` that existed only in the doc comment
  explaining why the call had been routed through the bounded chokepoint, and counted a
  fourth `fetch(` in `sw.js` that was likewise a sentence. Fixed by dropping whole-line
  comments before counting (`flatten`) — deliberately **not** a strip-from-`//`-to-end rule,
  which would truncate any code line holding an `https://` literal and silently stop
  counting whatever followed it, converting a false positive into a false negative.
  **Rule: a text-matching gate is measured against its own documentation before it is
  trusted, and it is verified by MUTATION — reintroduce the violation and watch it fire —
  not by observing that it is green.** A lint that has only ever been seen passing has not
  been shown to do anything. This sits beside the suite's standing rule about a cheap check
  shadowing an expensive one; here the cheap check was fooled by the expensive one's
  write-up.
  [D9, D23, `tools/net-lint.sh`]

- **AP30 — A durable record of someone else's assertion, with no way to re-ask.** State
  adopted from a remote source (a deployment document, a registry, a peer's claim) is written
  down permanently because it is *expensive to rediscover* — and then nothing ever
  rediscovers it. The write path is conditional on a divergence that, by construction, occurs
  exactly once; after it fires, the premise is never re-examined again for the life of the
  profile. **The trigger is therefore not the rare event the record was designed for — it is
  an ordinary mistake at the source**, which is corrected in minutes and yet becomes permanent
  on every client that happened to load during the window.
  *Incident (2026-08-27):* `peer_supersession` wrote `retired → replacement` durably on a
  warm-boot identity divergence, with **no delete path, no listing and no expiry**
  (`snapshot()` existed with zero callers). A single misconfigured `/entity-deployment.json`
  would be adopted by every browser that booted while it was live and would *survive the
  origin being fixed*, because the adoption branch never runs twice. Recovery was
  clear-site-data — brick-matrix cell **#6 at E5**, introduced one row down by the fix for
  cell #5, which is the worked example behind S-9.
  **Rule: a durable record derived from a remote assertion must carry the path back to that
  assertion.** Re-check it whenever the source is in hand and drop it when the source
  contradicts it; "we already asked once" is not a reason, it is the defect. Two corollaries,
  both of which are where the naive version goes wrong:
  **(a) absence of evidence is never evidence** — no document, an expired D23 deadline, or a
  document that declines to answer must change nothing, or a truncated file becomes a way to
  wipe good state; and **(b) revalidate strictly after adopting**, because a legitimate second
  divergence looks exactly like a stale record until the adoption path has written its half.
  **Incidents 2 and 3 (2026-08-28, found on live production by devops, verified here) — the
  same rule in the CACHE shape rather than the RECORD shape, and it is now the dominant form.**
  AP30 was earned on a record *derived from* a remote assertion; these are durable *copies of*
  a remote artifact, gated on `is_none()`, which is the identical "we already asked once"
  defect one layer down:
  - **The app bundle** (`src/views/games/mod.rs`): the catalog is refreshed once per
    window-open, the **bundle is fetched only when absent**. A returning profile therefore runs
    the app code it first downloaded, forever. Nothing upstream can compensate — `AppEntry`
    (`src/apps/format.rs`) carries **no content hash and no version**, so two publishes with
    entirely different app code produce byte-identical catalogs (measured), while the 58-byte
    bundle pointer is the only thing that moves. The stale blob still resolves because a
    content hotfix does not prune, so nothing 404s and the failure is **completely silent**:
    `boot_load: complete`, frame loop armed, every fetch `ok`, week-old app on screen.
  - **The deployment config** (`src/app.rs` ~1875): `deployment_config::fetch()` runs only when
    `durable.is_none()`. A warm boot therefore never re-reads `/entity-deployment.json` and
    **cannot report on its own routing document at any level** — no `applied`, no `unreachable`,
    no `not served`. The precedence rule it implements (persisted > fetched > build-time) is
    deliberate and defensible; *never looking again* is the part that is not.
  - **And the catalog on the line above is the tell: we already fixed this exact bug once.**
    Its comment reads *"this used to fetch only when absent, so apps added after the first
    visit NEVER appeared."* The repair landed on the file carrying **metadata** and not on the
    file carrying **the app**. A fix applied to one instance of a pattern, in a diff that
    stared at the second instance, is the strongest argument in this catalog for promoting
    this from an anti-pattern to a discipline.
  **Incident 4, found by the audit these three triggered, and it is the one that indicts us.**
  `content_site/cache.rs` exists *specifically* to answer "is my cache stale?" — its own doc
  comment says so, and `manifest_hash_hex` is documented as *"what a later revalidation compares
  the re-fetched manifest against to decide unchanged-vs-changed."* **There is no later
  revalidation.** `read_provenance` has two non-test callers and both only render the value in
  the UI; nothing compares `pinned_root_hash` to decide anything. The instrument was designed,
  built, correctly documented, and wired to a **label**. That is the same failure as this
  entry's original incident, where `snapshot()` existed with zero callers — **twice now we have
  built the mechanism and not connected it to a decision.** Building the instrument is not the
  work; *making something branch on it* is the work.
  **ROOT CAUSE, established by full inventory (`reviews/AUDIT-2026-08-28-CACHE-FRESHNESS-…`):
  every cache we designed AS a cache has a freshness model; the one that became a cache by
  accident has none, and it sits on top of the ones that are correct.** The durable entity store
  is conceived of as *the tree, the single source of truth* — true for state we own, and it
  quietly stopped being true when we began writing other people's artifacts into it under
  `/{me}/{foreign}/…`. Three consequences, all observed: **(i)** the presence check
  short-circuits the layers that are right — `Freshness::Mutable` → `no-store` is correct and
  its comment names this exact symptom, but it never runs because no request is issued;
  **(ii)** with no shared entry point owning the trigger, it is re-decided at every call site
  and the six consumers disagree; **(iii)** the wrong answer is the natural one to write —
  `if cached.is_none()` is correct for immutable content and the store gives no signal that this
  path holds someone else's mutable artifact. **This is why it recurs per app, per site, per
  feature, and why a reminder will not fix it: it is a missing abstraction, not carelessness.**
  The fix is to move the trigger to the layer that already owns freshness, and give the
  provenance ledger a reader.
  **PROMOTED to D24 on 2026-08-29**, in the change that landed both enforcement points it
  named — `an_app_republished_under_a_stable_identity_reaches_a_returning_profile` (observed
  **red** on the unfixed tree, green after) and `tools/foreign-cache-lint.sh` (observed firing
  on a deliberate violation). The lint that shipped guards the **module boundary**
  (`content_site::foreign_cache` is the only door to the per-artifact fetchers) rather than the
  guard *shape* this entry proposed: the shape rule is the true one and an unreliable grep, and
  **AP29** is this catalog's own entry for a gate that counts prose. The entry stays here as the
  incident record; **D24 is the rule.**
  [D16, `src/views/games/mod.rs`, `src/app.rs`, `src/apps/format.rs` `AppEntry`,
  `src/content_site/cache.rs`, `reviews/AUDIT-2026-08-28-CACHE-FRESHNESS-EVERY-COPY-OF-SOMEONE-ELSES-BYTES.md`,
  meta `ROUTING-2026-08-28-THE-BUNDLE-IS-FETCHED-ONLY-WHEN-ABSENT.md`]
  *Closed 2026-08-27:* `peer_supersession::{forget, revalidate, stale_against}` +
  `Peers::remove_and_wait`, gated by `a_supersession_the_domain_contradicts_is_dropped` —
  observed red on the unfixed tree, and falsified a second time by neutering only the
  *durable* half of the drop, which reds the count assertion alone. Cell #6 → **E1**.
  This is D9's third clause (*every persisted entity → writer / reader-at-boot / GC story*)
  applied to remotely-derived state, where the "GC" is a re-derivation rather than a sweep.
  Ladder position, honestly: **first instance**. The rollback pin's TTL (S-3) is the same
  shape but was caught by analysis, not by a bug, so this stays an anti-pattern and is not
  claimed as a discipline.
  [D9, D16, `AUDIT-REKEY-RECONCILE-2026-08-27` F2, `src/peer_supersession.rs`]

- **AP31 — A gate that is green by inheritance.** A test asserts against a build artifact
  that the target running the test does not produce. It passes for everyone who happened to
  run the other target first, and fails the moment someone runs it clean — so its record of
  greenness measures the developer's shell history, not the code.
  *Incident (2026-08-27):* `the_app_reports_the_build_it_is_running` (the C5 gate) asserts the
  app's logged build id against the `entity-build` stamp in `dist/index.html`. `make wasm` and
  `make wasm-release` both end in `./tools/build-stamp.sh`; the `make e2e-worker` build line
  did **not**, so the gate was green only on a `dist/` left behind by an earlier `make wasm`,
  and a clean `make e2e-worker` red it with *"tools/build-stamp.sh did not run."* It was
  reported green in the session that introduced it.
  **Rule: a gate must be run from a clean invocation of the target that owns it, before it is
  reported.** Where a gate needs a build step, that step belongs in *every* build path that
  feeds it — not in the one the author happened to use. The generalisation of the suite's
  existing `SKIP_BUILD=1` warning: that rule names one way to inherit a stale `dist/`; this is
  the same failure arriving through a target that never built the thing at all.
  [D10, `Makefile` `e2e-worker`, `tools/build-stamp.sh`]

- **AP32 — A diagnostic whose answer can come from the thing it is auditing.** A probe reads
  through a layer it is supposed to be checking, so the layer can satisfy the probe with its
  own stale copy. The report then says *"everything agrees"* — the most dangerous output a
  diagnostic has, because it is the one that stops the investigation.
  *Incident (2026-08-27):* the System Recovery version panel compares the running build with
  "what the origin serves now". `/index.html` is not a hashed asset, so `sw.js` routes it
  through `networkFirst` — which since C2 falls back to the **cached shell** when the origin is
  slow. On the exact failure being diagnosed (a stale worker, a slow or black-holing origin)
  the comparison would have been answered out of the cache under audit and reported
  *"you are running the current build."* A user acting on that stops looking, or clears site
  data on the wrong theory — the E5 action this work exists to prevent.
  **Rule: a diagnostic states the provenance of every value it compares, and reports
  INCONCLUSIVE rather than agreement whenever a value could have come from the layer under
  test.** Prefer a signal that cannot be spoofed by that layer: here the service worker's own
  `waiting` registration, which is definitive, needs no network, and is the observed form of
  the claim the console had previously only asserted. The panel now says "inconclusive — a
  service worker is controlling this page and its network-first leg falls back to the cache"
  whenever a controller is present, and reserves "current" for the uncontrolled case.
  *Related:* the suite's standing rule that a cheap check must not shadow an expensive one.
  This is its read-side twin — a cheap check shadowed by the cache it was meant to inspect.
  [D7, D13, `index.html` recovery console, `the_recovery_console_survives_a_blackholed_origin`]

- **AP33 — A report that is right for the case it was written for, reached by a case it
  misdiagnoses.** A user-facing string is authored against one concrete incident, is
  accurate and helpful for it, and then becomes the default answer for every *other* way of
  arriving at the same code path. It satisfies D13 — the state is reported, the surface is
  not blank, a gate asserting "it says something" goes green — while telling the user a
  false cause and, worse, an actionable-sounding false remedy. **A wrong report is more
  expensive than no report, because it is acted on.**
  **Scope, narrowed 2026-08-28 after the operator pushed on it — the correction matters and
  the rule survives it.** The challenge was *"if something's published, why is it saying that?
  Your lookup isn't right or something."* Checked: `err_no_manifest_foreign` is reachable from
  one place and only on `ResolveError::ManifestMissing`, so it **cannot fire for a site that is
  published and looked up correctly** — there is no lookup bug behind it, and the *observation*
  half of both strings was true in every state measured. **It is the cause-and-remedy half that
  is inherited from the case the string was written for.** State the observation freely; it is
  the diagnosis and the "go do X" that need the distinguishing fact in scope.
  *Incidents (both measured 2026-08-28, `a_pulled_demo_site_is_reported_not_blank`):* a
  publisher stops carrying the site a deployment names as its home — B-7, and a live shape,
  since two production domains declare a remote home named `demo`. Both surfaces report, and
  **both misattribute the cause.** A returning visitor gets
  `contentsite.offline_source_unreachable` — *"this site's source is unreachable, showing its
  cached outline"* — over a fully-navigable stale copy, when the origin in fact answered
  promptly with a 404 and the source is not unreachable but *withdrawn*; the user sees a
  working site that is a ghost. A first-time visitor gets
  `contentsite.err_no_manifest_foreign` — *"this site belongs to another peer and is probably
  hosted on its own domain — open it there"* — which was written for a shared link to
  somebody else's site (measured 2026-08-24) and is right there, but here names **this
  deployment's own publisher, on the origin the user is already looking at**, and so sends
  them away from the only place the site could ever have been.
  Neither can say the true thing — *the publisher of this deployment no longer carries this
  site* — because the resolve path collapses **withdrawn** (the origin answered, 404),
  **unreachable** (nothing answered) and **retired** (the identity moved) into one
  `ResolveError::ManifestMissing`, and does not know the home came from a deployment document.
  That is the tell: **the string is chosen by where the code failed, not by what the caller
  was trying to do**, so a second caller inherits the first one's diagnosis. The fix is
  therefore not a rewording — it is the missing distinction
  (`DESIGN-CONTENT-AVAILABILITY-AND-DEPLOYER-POLICY` §5.1), which is also the prerequisite for
  every deployer-configurable policy on the same path. A rewording without it just moves the
  guess.
  **Rule: when a report names a CAUSE or offers a REMEDY, the fact that distinguishes the
  cases must be in scope at the point the string is chosen — otherwise report the observation
  and not the diagnosis.** "Nothing is published at 'demo' on this origin" is always true;
  "…so open it on its own domain" is not. And when a gate asserts only that a surface speaks,
  say so in the gate — a green "it says something" must not be read as "it says the right
  thing." Same family as AP32 (a diagnostic that reports agreement it cannot support), one
  layer out: there the wrong answer came from the wrong source, here from the wrong case.
  **Corollary, operator 2026-08-28, and it corrects how D13 was being applied here: reporting
  the state does NOT mean reporting it to the user, on the surface, now.** *"Maybe the other ten
  sites are working fine, but one is off. Does that need to rise to the occasion and harass the
  user on every reload?"* A report has an **audience** and a **salience**, and both are part of
  the design: the home site of the deployment you are standing on is high-salience to the user
  (there is nothing else to look at); the eleventh entry of a directory listing is a log line
  for the operator. A surface that cries wolf trains people to ignore it, and then the one
  report that mattered is ignored too — **a report nobody reads costs more than no report.**
  Same prerequisite as everything else in this entry: the two cases are one `ManifestMissing`
  today, so nothing downstream can give them different salience even if it wanted to.
  [D13, D19, `src/i18n.rs` `contentsite.err_no_manifest_foreign` /
  `contentsite.offline_source_unreachable`, `a_pulled_demo_site_is_reported_not_blank`]
  **Incident 2 (2026-08-28) — the same defect in a LOG line, and it cost a reader two round
  trips.** `boot_load: remote home has no registered origin` (`src/app.rs` ~2384) fires
  whenever there is no deployment-config entry and `ENTITY_HOME_ORIGIN` is unset — **including
  on a completely healthy profile**, where the origin was persisted on an earlier boot and
  every fetch succeeds. The warning *states its own escape hatch* (*"it will only resolve if
  the origin is persisted/registered elsewhere"*), which is an admission that at the point the
  string is chosen the code **cannot tell the healthy case from the broken one** — exactly this
  entry's rule, one surface out from the user. A devops seat debugging a live production
  staleness bug read it as the cause and reported it as such; the actual defect was elsewhere
  (AP30 incidents 2–3), and the WARN was firing on the working path the whole time.
  **The corollary above is not only about user-facing surfaces: a log line has an audience and
  a salience too**, and its audience is the person debugging an incident at their least
  skeptical. A warning that fires on the healthy path trains readers to ignore it — and the
  cost is not the ignoring, it is the two hours spent chasing it the one time they don't.
  Either establish the distinction (is the origin resolvable *now*?) before choosing the
  string, or drop it to `debug!` and state the observation without the diagnosis.
  *Closed 2026-08-29, by the first route.* Boot now performs the authoritative read
  (`get_entity_async` on the origin path — **not** the sync mirror, which is unsubscribed for
  that prefix at boot and would answer `None` for every profile, turning the fix into the same
  bug with more code) and picks between a `debug!` *"registered from an earlier boot"* and a
  `warn!` *"NO registered origin… this home cannot resolve."* The warning kept its teeth: the
  broken case still warns, and it now says something that is only true when it is true. Note
  which half of AP33 this closes — the **log** incident, where the distinguishing fact was
  cheaply available. The **user-facing** half (incident 1) still cannot be fixed by rewording:
  it needs the withdrawn/unreachable/retired split, which does not exist yet.

- **AP34 — A gate whose POPULATION excludes the failing configuration.** Not a weak
  assertion and not a stale artifact: the assertions are right and the rig is clean, but the
  one variable that decides the outcome is held at a single value across every run. The gate
  is then green *by construction*, forever, and its greenness is evidence about the value
  that was chosen and about nothing else.
  *Incident (2026-08-28):* every WebRTC gate in this repo was Firefox↔Firefox —
  `browserName: "firefox"` in all six `tools/e2e/webrtc-rung1/spike_*.py` and
  `selenium/standalone-firefox` as the only image. Engines disagree about the data channel's
  `maxMessageSize` by four orders of magnitude, and Firefox is the value at which the app's
  oversized writes cannot fail. So when a real Android(Chrome) → desktop(Firefox) transfer of
  a 6.5 MB photo stalled after one progress line, `make e2e-webrtc-file` was green — and had
  never had the *ability* to be otherwise. **No WebRTC behaviour of this app had ever been
  executed against a second engine.** Adding one (`ENGINE_A`/`ENGINE_B`, `img_for`) reproduced
  the operator's stall on the first run, and the negotiated ceiling printed **262144 on both
  sides** where Firefox↔Firefox prints ~1 GiB.
  **Rule: when the substrate has more than one implementation, a gate that fixes the
  implementation is a gate over one population — say so where it is defined, and cover the
  MIXED pair, not a second monoculture.** Mixed is the load-bearing part: two Chromes agree
  with each other exactly as two Firefoxes do, and a phone talking to a laptop is neither.
  Distinct from AP31, which is green because of an artifact another target left behind — this
  one is green on a clean run of its own target, which is why nothing in the AP31 rule catches
  it. Same shape as D10's "green tests ≠ a working app" one level down: there the loop was
  missing, here the *variable* was.
  [D10, `Makefile` `e2e-webrtc-file-crossengine`, `tools/e2e/webrtc-rung1/rung1_repro.sh`
  `img_for`, `spike_meet_then_chat.py` `caps_for` — which prints the engine the grid actually
  started, because a mixed run that quietly came up same-engine would re-earn this entry]

- **AP35 — An undeclared ceiling at a layer boundary, with the error thrown away.** A lower
  layer has a hard limit the layer above cannot see, the caller is sized against a *different*
  budget that happens to share the word "frame", and the write that violates it fails into a
  discarded `Result`. Each of the three is survivable alone; together they convert a size
  violation into an unbounded wait, which is the one failure with no report and no timeout.
  *Incident (2026-08-28):* the broker's port→data-channel pump was
  `if open { let _ = dc.send_with_u8_array(&bytes); }` — one `send()` per port message, no
  size check, no `maxMessageSize` read anywhere in the codebase, no fragmentation. Above it,
  `file_offer.rs` sizes a pull at `GET_BATCH_SIZE` = 16 chunks ≈ **4 MiB**, documented as
  *"well inside the 16 MiB frame budget"* — the **entity protocol's** frame budget, not the
  transport's, which a Firefox↔Chrome pair negotiates at **262144**. `CHUNK_SIZE` is
  `256 * 1024`, so even one chunk plus its envelope was over. The throw was discarded, so the
  puller waited on a response that could never come: *"starting chunk 0 out of 26"*, then
  silence, forever.
  **Rule: a transport either carries what it is handed or reports that it cannot — never
  both-and-neither. Prefer making the limit invisible to the layer above (fragment) over
  publishing it upward (a size the caller must respect), and NEVER discard the `Result` of a
  platform write.** The fix here could be transparent precisely *because* the layer above is a
  byte stream — `PortReader` feeds `entity-wire`'s 4-byte length prefix and already carries a
  `leftover` — so message boundaries on the carrier had no semantics and splitting needed no
  header, no reassembler and no wire change. **Check that before reaching for a protocol
  version.** A failed send is not a lost message but a hole in a stream, so it closes the
  channel: an unrecoverable transport error the layers above already report beats a hang they
  cannot see. And the corollary the operator's report earned: *do not fix this by lowering a
  constant* — a number that happens to work on one pair is how it comes back on the next.
  [D9, D14, `bindings/wasm-worker-proxy/src/webrtc_session.rs` `PortPump` /
  `outbound_piece_size`, `make e2e-webrtc-file-crossengine` (mutation-checked: red on the
  unfixed pump, green on the fixed one, same rig)]
- **AP36 — One predicate standing in for two questions.** A single test guards both *may we
  ACQUIRE this?* and *may we ADOPT it?*, and answering the first with the second silently
  withholds everything the acquisition was also good for. The tell is a guard whose *name*
  answers one question and whose *position* answers another.
  *Incident (2026-08-30):* `deployment_config::fetch()` sat inside boot's `!home_is_local`
  gate. `home_is_local` is a correct and load-bearing answer to *may we adopt the document's
  home?* — an empty `peer_id` is the system-peer sentinel, so adopting it every boot would
  overwrite a user's deliberate local home. It is not an answer to *may we read the domain's
  document at all?*, and because it stood in for one, a local-home profile never adopted a
  moved origin (map-B1's repair reached the deployed domains and nobody else), never
  revalidated a supersession record (boot audit B-5), and emitted **no line at any level**
  about the document — which is why a devops seat could look, see nothing, and be right.
  **This is the third instance of the same shape in one audit thread**, which is what makes it
  a catalog entry rather than a bug: `put_if_absent` stood in for *did the user set this?*
  (AP30's origin half), a presence check stood in for *do I hold the CURRENT bytes?* (D24), and
  `home_is_local` stood in for *may we read?*. Each time the fix was the same move — put the
  guard on the **decision**, never on the **acquisition** — and each time the wrong version
  read as the conservative one.
  **Rule: when a guard is about to skip work, name the question it answers and check that
  every consequence of skipping is downstream of THAT question.** The consequences here —
  origins, revalidation, reportability — were about other peers, other records, and the
  operator; none of them were about this profile's home.
  [D24, AP30, `app.rs` `boot_load` (the fetch is unconditional; `!home_is_local` guards only
  the adoption), `make e2e-worker T=a_local_home_profile_reads_the_deployment_document`]
- **AP37 — A gate's diagnostic half, unmaintained by the compiler.** A gate greps the captured
  log for a string it does not assert on — a `println!`, a counted "signature" — and when the
  string moves, nothing goes red. The gate keeps passing while the artifact it exists to
  produce quietly becomes empty.
  *Incident (2026-08-30):* the re-key gate collects and prints every
  `remote home has no registered origin` line, described in its own comment as *"the
  reproduction signature, the same shape as the operator's captured console"*. C1 rewrote that
  line to `has NO registered origin` in a different commit. The filter is
  case-**sensitive** and is a `println!`, so the suite stayed green at 38/0 and the gate began
  reporting **zero** occurrences of the thing it was built to characterize — for the incident
  it was built for.
  **Rule: a string a gate reads is a coupling whether or not it is asserted. Either assert it
  (so a rename reds) or match on the part that cannot move; and when you change a log line,
  grep the test tree for it in the same diff.** Same family as AP31 — a gate that says
  something it has not earned — but the failure is in the *reporting* half, which no
  pass/fail rule reaches.
  [D18, `tests/e2e_worker.rs` `rekey_scenario`]
- **AP38 — An anti-vacuity guard that hides the gate behind it.** A gate asserts its staging
  first (*"this run really was the scenario"*) and its product claim second, which is correct
  ordering — and it means the **falsification run only exercises the guard.** The product
  assertion, the one the gate exists for, is never observed red, so it is a claim with a
  passing test in front of it.
  *Incident (2026-08-30):* `a_moved_origin_reaches_a_returning_profile` was neutered by
  reverting `adopt_deployment_origin` to `put_if_absent` semantics. It went red — on
  *"the boot never reported `Updated`"*, the anti-vacuity assertion. Only after suppressing
  that assertion and re-running the same neuter did the real one speak: *"the returning profile
  stayed on the old one. Player body was 101 character(s)"* — the stale bundle, still on
  screen. Had the falsification stopped at the first red, a gate whose content assertion was
  (say) matching an empty string would have looked mutation-checked.
  **Rule: a gate with N independent assertion tiers needs N falsification runs, or you have
  only observed the outermost one. Record which tier each red came from.** The cheap technique
  is to short-circuit the outer assertion (`true || guard`) and re-run the SAME neuter, so the
  two reds are known to come from one defect.
  [D18, AP31, the falsification log in
  `docs/plans/PLAN-2026-08-29-THE-CHANGE-MAP-EVERY-CHANGE-AUDITED-AND-ITS-GATES.md` §7]
- **AP39 — A test double that misreports WHICH failure it is producing.** A stub returns a
  plausible-looking error of the wrong *kind* — the right words in the wrong variant. It is
  invisible for exactly as long as the code under test collapses those kinds together, and it
  becomes load-bearing the moment somebody stops collapsing them: the new behaviour is then
  untestable, or, if the assertion is written to match the double rather than reality, the
  suite proves the opposite of the truth and looks rigorous doing it.
  *Incident (2026-08-30):* `FixtureBinSource` (`http_poll`'s own test source) mapped a path the
  fixture does not carry to `PollError::Decode(format!("404 {url}"))` — a string that says 404
  while the variant says *"the bytes did not decode"*. `PollError` distinguishes those on
  purpose, in a doc comment that says *"collapsing the two is how 'withheld' and 'unreachable'
  arrive as the same value"*. Harmless while `resolve_closure_via` discarded the error anyway;
  the moment C2 made the decode point preserve it, the new `an_origin_that_answered_404_is_a_
  withdrawal_not_an_outage` failed **against correct product code**. Its sibling double in
  `foreign_cache` had it right (`PollError::NotFound(404)`), which is how the diagnosis was one
  minute rather than an afternoon.
  **Rule: a double must be honest about the distinction under test, and about every distinction
  the type it fakes draws — a stub is an implementation of a contract, not a convenience.**
  When a variant exists to be told apart from another, no fixture may produce the two
  interchangeably. Check the doubles before changing the meaning of an error type, and prefer
  one shared constructor (`poll_error_for_io`) over per-fixture guesses.
  [D18, `src/content_site/http_poll.rs` `FixtureBinSource`, `poll_error_for_io`]
- **AP40 — Splitting a collapsed value, and giving the strongest claim to the widest bucket.**
  A change exists to stop conflating outcomes; it introduces distinct variants; and its **own
  new mapping** then hands the most specific, most actionable statement to the `_ =>` arm. The
  split reads as a correction and ships the same defect one boundary over — with the extra
  hazard that everyone now trusts the value, because it is typed.
  *Incidents (2026-08-30, both found auditing the session that introduced them, hours apart):*
  **map-B3** split *"is there a deployment document"* into four states and then mapped **every**
  non-2xx to `NoDocument`, whose message is *"this deployment runs on build-time defaults **by
  choice**"* — so a CDN 502 reported the deployer's *intent*. **map-C2** split *"the site is not
  here"* by whether the origin answered, and then treated every answered-404 on a foreign site as
  a withdrawal — regressing the `?site=` deep-link case (*we asked a host that never had it*)
  that the message family it replaced had been earned on. In both, the rule was already written
  down in this repo (`PollError::NotFound`: *"Only 404/410 map here, deliberately"*) and imported
  incompletely.
  **Rule: in a split, the narrowest input gets the strongest claim and the default arm gets the
  weakest. Enumerate the inputs that reach each arm and ask what each one licenses you to
  say — a `_ =>` that produces a specific diagnosis is the smell.** And prefer an evidence test
  over a status test: C2's fix is *"assert a withdrawal only where something vouches this origin
  is where that peer publishes"* — a held copy, or the deployment's own declaration — which is a
  fact we have, where "it answered 404" is not.
  Same family as AP33 (a report right for the case it was written for, reached by a case it was
  not); AP40 is the *manufacturing* step, and it is where to look first after any "we now
  distinguish X from Y" change.
  [D13, AP30, AP33, `deployment_config::classify` + `declares_no_document`,
  `views/content_site/model.rs` `error_output`,
  `a_withdrawn_foreign_site_we_never_held_is_not_claimed_as_a_withdrawal`]
- **AP41 — A timing-dependent read, CACHED as if it were a fact.** A surface reads durable
  state once, at construction, through an accessor whose answer depends on *when* it is
  called; stores the result in memory; and never reads again. The surface then spends its
  entire session acting on a value it invented, while the real one sits in the tree — and the
  failure is *silent by construction*, because a default is a legitimate value and nothing
  distinguishes it from an answer.
  **Do not assume this is confined to the arm where the accessor is obviously wrong.** The
  incident below was filed as Worker-only on exactly that reasoning, and a falsification run
  refuted it: on the Direct-IDB arm the store fills from IndexedDB *while the constructor
  runs*, so the same read is **racy** rather than reliably correct — measured as a 1-in-3
  flake, and neutering the fix turns **both** arms red. **Structural on one arm, intermittent
  on the other** is the general shape, and the intermittent one is the shipped default, which
  is where it hurts. **The "control" that cannot fail is the thing to falsify first** — this
  one was written as a no-op assertion and turned out to be the finding.
  **This is NOT the known "subscribe the prefix you read" rule
  (`feedback_worker_cache_get_needs_subscription`), and applying that rule would not have
  fixed it.** Two reasons, and both had to be true: the read happens *before* the subscribe
  in the same synchronous block (`SiteOverlay::new` reads at `:75` and subscribes at `:88`;
  the window factory does the same), and `observe` is asynchronous, so **even a correctly
  ordered subscription cannot make a same-tick read authoritative.** Subscription fixes the
  *next* read. The bug is that there is no next read.
  *Incident (2026-08-30):* `ContentSiteModel::initialize` read the persisted
  `ContentSiteState` — and the `home_site` config it derives its fallback from — with the
  sync `Peers::get_entity`. On the Worker arm both answered `None`, measured with the
  `audit-worker-reads` lamp on a `warm-durable` boot. Every returning reader on `?worker=1`
  was silently put back on the **build** default (`demo`, peerless), which renders
  *"No site manifest at 'demo' (peer: …)"* — an availability defect that presented for a day
  as a narrow re-key regression, because the only two gates that read navigation state back
  on that arm were re-key gates.
  **Rule: if a read can be empty for reasons of timing rather than of truth, you may render
  from it but you may not RETAIN it. Either read authoritatively (`get_entity_async` /
  `tree_listing_async`, subscription-independent on both arms) or re-read when the source
  becomes readable — and say which of the two you did.** The corollaries the fix is built
  from, because the naive repair introduces two new bugs: **a read that FAILS is not an
  answer** (keep what you have — AP30 corollary (a); `Hydration::Unheard`), and **a write
  that lands during the round-trip is newer than it** (do not drag the user backwards —
  `nav_generation`). And do not report the outcome as a `bool`: *"we restored your
  location"*, *"you never had one"*, *"we could not tell"* and *"you moved first"* are four
  facts, and collapsing them is AP40 (the first draft of `Hydration` did exactly that).
  **Counter-example worth keeping in view:** `SettingsModel` has the same cold read and is
  *not* broken, because it re-reads on every render — it self-heals by accident, which is
  why "it works in Settings" was never evidence about anything else.
  **Eight window models share the retaining shape** (`content_site`, `shell`, `entity_tree`,
  `knowledge_base`, `query_console`, `execute_console`, `peer_connections`, `chain_trace`).
  `content_site` is fixed; the **Shell is measured red on the same shape** — a re-opened window
  starts at the default working directory — which is what promotes the inventory from a code
  reading to a second confirmed instance. The remaining six are named, unmeasured open work.
  **What makes this reachable for window models is not obvious and took a wrong turn to find:**
  a reload restores **no** windows at all (`app.rs`: *"No default window spawn. A Chrome/Full
  boot opens ZERO windows"*), but **window ids restart at 1**, so a re-opened window inherits
  the previous session's `workspace/windows/{id}/state` — and the `surface=window` deployment
  shape re-spawns window 1 on every boot and reads it every time.
  [D13, D15, D16, AP4, AP30, AP40, `views/content_site/model.rs` `hydrate_durable`,
  `a_returning_reader_is_still_on_the_page_they_left_on_the_worker_arm`,
  `docs/plans/AUDIT-WORKER-ARM-NAVIGATION-2026-08-30.md`]

- **AP42 — A durable slot keyed by a REUSED id, decoded without asking what wrote it.** Two
  facts that are individually fine compose into a silent cross-read: an identifier is handed
  out again after a restart, and the state it keys outlives the thing that wrote it. The
  decoder then reads whatever is in the slot **by field name**, so an entity written by a
  different *kind* of thing is adopted field-by-field wherever the two happen to agree — no
  error, no log line, and a default is indistinguishable from an answer (AP41's silence, from
  the other direction).
  *Incident (2026-08-30, latent — measured, not yet observed in production):* window state
  lives at `workspace/windows/{id}/state`; `WindowManager::new` restarts `next_id` at 1 every
  session, and **a reload is not a close** — only `Action::CloseWindow` removes window state
  (`app.rs:3149`, D9 satisfied *there*) — so the entity at window 1's path on this boot was
  written by whatever window held id 1 on the last one, **of any type**. The Entity Tree and
  the Knowledge Base both persist `expanded_paths` and mean different things by it (entity
  paths vs. doc-tree folders), and the Entity Tree adopted the KB's: measured at the adoption
  site, `pending_expand_restore == {"guides", "guides/testing"}`.
  **Report the severity honestly, and this one is LATENT, not live.** The adopted set is inert
  today for two reasons that are coincidences rather than guarantees: KB folder paths are
  relative while tree paths are `/{peer_id}/…`, so no key matches; and `restore_expanded` is
  **additive** (it sets `expanded = true` on a hit and never collapses), so a non-matching set
  is a no-op. Neither is enforced by anything. `expanded_paths` is also the *only* field name
  shared across the eight decoders today — which is a fact about today's field names, not a
  property of the design.
  **Rule: when a slot's key can be reused, the thing in it must say what it is, and the reader
  must check.** Here the discriminator already existed and was being thrown away — every
  window state is stamped `Entity::new("app/state/{type}", …)` — so the fix was to read the
  field, not to invent one: `if entity.entity_type != STATE_TYPE { return <no persisted
  state> }` in every decoder, with the type literal promoted to a `STATE_TYPE` constant
  so `to_entity` and `from_entity` cannot drift apart. Note the answer for a foreign entity is
  the model's **"no persisted state"** value, which is not always `Default` (the Shell's is
  `initial("")`).
  **Corollary, and it is the half a type guard CANNOT deliver: two window types must not share
  one state type.** The guard separates readers that disagree about their type; it is blind to
  two windows that agree. `ProgramsWindow` persisted `AppViewState` under the Apps window's
  `app/state/games_view` at its own `window_state_path`, so a Programs window inheriting an
  Apps window's id decoded it and read a `selected` naming another set's app as a program key.
  Programs now carries `app/state/programs_view` and the shape is shared through
  `from_entity_as` / `to_entity_as` — **a shared codec is fine, a shared slot is not.** The
  gate asserts type distinctness *before* the cross-matrix, because the matrix skips
  `writer == reader` and was therefore green while the two were merged: **the first
  falsification of this finding failed to go red, and that is what exposed the hole.**
  **Count from the roster, not from a grep.** The decoder list was first derived with
  `grep 'Entity::new("app/state'` over `src/views/` and came up **three short** — it missed
  `content_site` and `games`, which had already promoted their literal to a constant (so the
  literal was not there to find), and `programs`, which writes through another module's codec
  entirely. The census is `grep -rln window_state_path src/views/`: **eleven** window types,
  and the gate asserts `rows.len()` so the next omission fails rather than passes quietly.
  **What this does and does not buy.** It converts *silently adopt another type's fields* into
  *behave as if there were no state* — it does **not** stop the last window to hold an id from
  overwriting the previous holder's state, which is ordinary loss and unchanged. It is
  therefore a correctness floor, not the resume semantics; type-scoping the path
  (`windows/{type}/{ordinal}/state`) is the separate, larger change that would also make
  resume order-independent, and it is a product decision with a migration cost.
  **Why it was worth doing FIRST:** the six remaining AP41 repairs make these reads reliable.
  Repairing a read whose key still collides makes a wrong answer arrive *dependably* — the
  same shape as every defect in this thread, a mechanism working correctly and delivering
  something nobody decided.
  *Process lesson, and it cost the first version of the gate:* **the first assertion was
  written against a derived view and passed while the defect was fully present.**
  `state_snapshot().expanded_paths` reports what is expanded in the *loaded* tree, which in a
  unit test with no entities loaded is empty no matter what was read. **Assert at the adoption
  site, not at a render-derived one** — a rendered assertion is right for a behavioural gate
  (AP31) and can measure nothing at all in a unit test whose fixture never populates the view.
  *Two things the same review turned up in the surrounding data model, neither a defect:*
  (a) the **stated convention is sound and was being applied** — `app_paths.rs` declares
  `app/{app_id}/workspace/…` for per-window state and `app/{app_id}/settings/…` for global
  configuration, which is why `SettingsModel` is not an exception: it holds theme, language
  and the like, is a *stateless typed accessor* over one global entity, and ignores its own
  `window_id`. Two Settings windows showing one configuration is the convention working, not
  a special case. (b) **`window_results_path` has no writer and no reader** — its only uses
  are its definition, its unit test, and the `CloseWindow` removal, so the close path deletes
  a slot nothing creates. Left in place deliberately: ADR-0027 authors published commits fresh
  at the release boundary, so `git log -S` over this history **cannot** establish that no
  earlier build wrote one, and a cleanup for a legacy slot is cheaper than stranding it.
  [AP31, AP39, AP41, D9, `window.rs` `STATE_TYPE` / `next_id`,
  `no_window_state_decoder_adopts_another_window_types_entity`,
  `a_foreign_window_states_expanded_paths_are_not_adopted`,
  `views/programs/mod.rs` `PROGRAMS_VIEW_TYPE`]

- **AP43 — "I wrote it" used as a proxy for "it changed": an IDEMPOTENT WRITE IS NOT AN
  EVENT.** A surface signals its own re-render by persisting something and letting its own
  subscription come back to it. That works right up until the write is byte-identical — and in
  a **content-addressed** store an identical put at the same path is not a change, emits no
  event, and is silently indistinguishable from a write that never happened. Everything the
  surface did that was *not* in that entity is then stranded: computed, held in memory, and
  never drawn.
  **The tell is a surface that displays state it deliberately does not persist.** If what the
  user sees is a pure function of the persisted entity, this cannot bite — no change, nothing
  to redraw. The bug lives exactly where the two diverge.
  *Incident (2026-08-30):* the Shell's scrollback is session-only by design (`to_entity`
  persists `wd`/`history`/`draft`, never scrollback — refresh wipes it, the conventional shell
  behaviour). `ShellWindow::handle_action` returned `dirty = true` for a submission and then
  only called `save_state`, relying on the watch it had installed on its **own**
  `window_state_path`. But `record_submit` skips a consecutive duplicate, so re-running the
  last command changes `history` not at all, clears an already-clear `draft`, and produces the
  identical entity — no event, no rebuild, and the `<pre>` keeps the empty-scrollback
  placeholder while the model holds the rows. **Measured natively on the Direct arm:** first
  command dirties, a different command dirties, the repeat does not, with 7 rows sitting in
  the model.
  **This presented for a day as *"a Shell opened after a reload renders nothing"*, and that
  framing is what made it look undiagnosable.** Warm-vs-cold is not the axis; *did the
  persisted entity change* is. It correlates with a reload because a cold shell's first
  command always grows an empty history, while a returning shell starts with history restored
  — and the natural response to "nothing happened" is to type it again, which is guaranteed to
  be the duplicate case. The earlier hypothesis (*"something re-reads state and wipes the
  scrollback"*) was wrong in the informative way: **nothing wipes anything. The rows are all
  there. The section simply never rebuilds.**
  **Rule: a surface that displays unpersisted state marks its own watch dirty; it may not
  infer a redraw from a write.** The subscription stays for writes made *elsewhere* (an async
  `exec` completing) — it just cannot be the trigger for output the surface produced itself.
  **Do not generalise this into "mark dirty everywhere".** Checked rather than assumed: six
  windows persist in `handle_action` without marking dirty, and the other five are correct
  because everything they render is either persisted (`entity_tree`, `knowledge_base`,
  `chain_trace`, `settings` — display is a pure function of the entity) or arrives on a
  *different* subscription that does see a real change (`query_console` /`execute_console`
  render results out of the event-log prefix, where every entry is a new sequence-numbered
  path). The Shell is the only surface here with a substantial session-only display buffer.
  **Tension worth holding in view, because the same property is load-bearing in both
  directions:** D24 records *"`Unchanged` writes nothing and flips nothing dirty"* as a
  **feature** — on the Apps surface a spurious dirty restarts a running app. That is the same
  no-op-write behaviour that lost the Shell's output. Neither is wrong; the lesson is that
  "nothing changed in the store" and "nothing changed on screen" are different questions, and
  a surface has to answer the second one itself.
  [D9, AP31, AP41, D24, `views/shell/mod.rs` `handle_action`,
  `a_submission_that_changes_no_persisted_state_still_rebuilds`,
  `a_shell_spawned_after_a_reload_still_shows_what_it_prints`]

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
deployment posture for the lead release (the startup **surface** —
chrome / window / site — plus the granular `site_mode`/`peer_creation_enabled`
ARE the E1 config mechanism, not a v1 architecture fork: the system peer always
exists; the surface + posture differ in what's exposed and whether the overlay
is forced. The old opaque `full`/`site`/`strict-site` *profile* presets were
removed — set the surface directly);
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

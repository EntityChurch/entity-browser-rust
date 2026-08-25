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

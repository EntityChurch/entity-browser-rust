# Doctrines — task-oriented procedures for the browser substrate

> **Status:** Living document. The procedure layer that sits on top of the
> discipline charter (`DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md`).
> Codified after a multi-peer boot / site-robustness audit reset — when the
> decision was made to stop throwing fixes at the wall and codify procedure.
>
> **Provenance.** This is the browser-substrate sibling of the Godot team's
> `DOCTRINES.md` (`../godot-entity-core-rust/docs/`), which they
> codified the same day after their own ingest-hang audit. **The skeleton
> transfers verbatim** — two doctrines (Feature / Audit), both ending in a
> learning-extraction ratchet, both binding the disciplines as checkpoints.
> **The substrate steps are ours** — because our recurring failure mode is not
> theirs (§0.5). Where their step is about the shared entity-OS layer it
> transfers; where it is about their runtime (Godot signals, `PeerOpFuture`),
> we replace it with the browser equivalent (the arm matrix, the worker cache,
> the rAF loop, the service worker, the WASM↔JS↔Worker handoff chain).

---

## §0 What this is — disciplines vs doctrines

We already have **disciplines** (D1–D16, in
`DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md`). They are **invariants
the code obeys** — the *what*. This doc is the **doctrines** — the *how*: the
step-by-step procedure you open when a specific kind of work lands.

| | Disciplines (D1–D16) | Doctrines (Feature / Audit) |
|---|---|---|
| **Form** | Invariants. "Every `Closure` has a drop path." "Decide the arm from the bound peer, never the primary." | Procedures. "Step F1: write the state machine. Step A1: trace a value before you theorize." |
| **When applied** | Continuously. Every diff, every review. | At task-start. When you get the input "build X" or "Y is broken." |
| **Location** | The charter + AGENTS.md inline anti-patterns (auto-loaded). | This doc (read at doctrine-start). |
| **Failure mode** | Code drifts. The nine review questions catch it. | Process drifts. The doctrine's own ratchet catches it. |
| **Growth** | A new discipline ratifies when a bug earns it (2 incidents, different shapes). | A new step ratifies when a doctrine run surfaces a gap. |

**The disciplines bind both doctrines.** Both have steps that invoke specific
disciplines as checks (the nine review questions, D9 accounting, D13 surface
decided up front, D15 arm decision). **Don't duplicate disciplines into this
doc — reference them by number.**

**Both doctrines end with a discipline-update step.** If the work taught us
something the disciplines don't capture, we update them *in the same session*.
The charter is the canonical spec; an unsynced lesson rots.

**Why this exists, in one line:** when the next bug lands or the next feature
is asked for, the answer can't be "think hard and hope." It has to be: *open
this doctrine, you're at step N, the next step is M.* No guessing, no
improvising, no head-scratching.

---

## §0.5 Why now — our recurring failure pattern (the corner we keep landing in)

Every doctrine is earned. The disciplines were earned by shipped bugs (the
charter cites each). The *doctrines* are earned by a shipped **process** bug:
a loop we keep running. Name it precisely, because the whole point is to break
*this specific cycle*, not a generic one.

**The cycle, observed across this whole arc (publish/site-cache/boot-config):**

1. A change ships **green** — `make test` (475 native · 17 peer-int), clippy,
   `make wasm`, `make e2e-worker` 3/3. The internal signals all say done.
2. It breaks on the **Worker arm**, with **real multi-peer / persisted /
   multi-tab state** — a configuration *the suite never exercises*. Native
   tests and the single-peer Direct path pass straight through it.
   (Backend-delete bug; "No sites yet" per-peer writer divergence; worker
   cache reads stale `None`.)
3. The symptom is **diffuse and mislabeled** — "it's slow," "it reset," "no
   sites," "it's crazy" — because the failure surface is **silent**: a silent
   Direct fallback when worker-OPFS bootstrap fails, a silent stale cache, a
   silently-ephemeral second tab, a frozen rAF loop that *looks like* a connect
   timeout.
4. We then **theorize about the substrate** instead of tracing a value.
   (`feedback_trace_values_dont_theorize_substrate`: we theorized a
   flush/subscription race for *many rounds*; one `tracing::debug!` showed
   `found=true` 877× — the bug was our own logic.)
5. The user finds the edge case in their real browser and reports it. We react.
   **Go to 1.**

That is the cycle the user named: *"waiting for me to find every goddamn edge
case and come back and tell you to fix it — that's not working."* And the cost
compounds: **adding a feature has been making us weaker** (a new bug per
feature) instead of stronger.

**The five levers that break it — and which doctrine step owns each:**

| The recurring failure | The lever | Owned by |
|---|---|---|
| Green ≠ working; breaks on the Worker arm with real state | **The arm × state matrix** is a first-class design *and* test dimension. "Tested" = native **and** Worker-e2e **and** (for lifecycle/persistence) a real multi-peer/persisted profile. | F2-arm, F5, A6-C, D10, D15 |
| Diffuse, mislabeled symptoms | **Silence is the enemy.** No silent fallback, no silent stale read, no silent ephemeral. Every degraded mode surfaces loudly and recoverably. | F3 (D13), A3 telemetry-gap map, D13/D16 |
| We theorize about the substrate | **Trace before you theorize.** One `tracing::debug!(found=…)` and a re-run *before* any substrate hypothesis. | A1 (our prime) |
| Green dist ≠ what the user runs | **Verify through the real delivery path** — the service worker, the live bundle hash, each WebView runtime. | F6, A12 |
| The feature made us weaker, not stronger | **The ratchet.** Every feature and every audit ends by feeding what it taught back into the disciplines + this doc. | F7, A10/A11 |

These map onto the disciplines we already have — the doctrines just make us
*run them in order, on demand,* instead of remembering them ad hoc.

---

## §1 Feature Development Doctrine

**Input:** a new feature, window, panel, helper, substrate verb, site/cache
surface — any non-trivial change.

**Output:** the feature shipped, tested on the **arm and state that matter**,
every degraded mode **surfaced not silent**, and the system understood
incrementally better than before.

### Step F0 — Intake

- Restate the ask in one sentence. If you can't, ask **one** clarifying
  question (`AskUserQuestion`).
- Note who asked, when, the success criteria — even informally. "User wants a
  per-peer site directory rail that doesn't say 'No sites yet' for a backend
  peer" is enough.

### Step F1 — State machine on paper

Write every state the feature's entity (or process) can be in, every
transition, and **what the user sees in each state**.

- A state with **no surface** is a D13 violation by construction. *Fix the
  design before the code.*
- A **silent transition** (e.g. `status: loading → ready` with no observable
  field) is anti-pattern AP11 (defensive code that lies) / a future "it just
  spins" report. Give it a surface now.
- Two states and one transition is fine. A state machine you *can't write down*
  because "it's just one function" is the warning sign.

### Step F1.5 — Map the handoff chain (substrate-native)

Our bugs live at the **boundaries**, so name every hop the change crosses and
**what is observable at each:**

- **WASM ↔ JS** — any `Closure`, `JsValue`, DOM handle, `js_sys::Promise`.
  (Two heaps; D12.)
- **main thread ↔ Worker** — any `postMessage`, control-port transfer, proxied
  SDK op. (D14; the wire moves, not copies.)
- **peer ↔ peer** — any `execute("entity://{pid}/…")`, `ws://`, `xworker://`,
  `memory://` hop. (Transport table in `IMPLEMENTATION-ARCHITECTURE.md`.)
- **store write ↔ store read** — *which store* writes, *which store* reads. On
  the Worker arm these can be **different stores per peer** — the
  divergence class behind "No sites yet" (`Peers::writer_handle_for`).

For each hop: what crosses it, and *can you see the value on the other side?*
If a hop is invisible, that is where the next "it's crazy" bug will hide — give
it a trace point in F3.

### Step F2 — The nine review questions on the design (before code)

Run all nine from the charter §4 **before writing code**. They are short:

1. **Which layer?** L5 / L4 / L3 / L2.5-kernel / browser-OS / host-OS.
2. **What kernel service does this consume / reimplement?** (D1.)
3. **Capability surface?** Gated, held-cap set explicit, fails closed (D3).
4. **Failure mode if the kernel misbehaves AND if this code misbehaves?**
   Symmetric (D7). **If "the user can't tell anything is wrong" is a possible
   answer, you need a D13 surface — see F3.**
5. **Accounting?** Every `Closure`/listener/`Rc`/cache → drop path at the same
   change; every persisted entity → writer / reader-at-boot / GC story; every
   per-peer cache → eviction at close (D9, D12).
6. **Does the test cross the real loops?** Cross-reload, real-store, the right
   **arm** and the right **runtime** (D10) — expanded in F2-arm.
7. **Which arm?** Is any Direct-only API (`sdk()`, `peer_shared`, sync
   `delete_peer`, `peer_context`) reached without routing via the **target
   peer's** owning SDK? (D15 — our #1 footgun, AP4.)
8. **Can this panic in a frame?** If so it kills the rAF loop and freezes the
   app (D13, AP3). Reschedule before the fallible section.
9. **What persists, where, with what fallback and cold-return story?** (D16 —
   "I leave, I come back three weeks later, did it save my shit?")

**Output of this step is a one-paragraph design note** (commit body or scratch
file) naming: the layer, the kernel surface, the **arm(s)** it runs on, the
**D13 channel**, the D9 story, the test category. **The note is the forcing
function. If you can't write it, you don't have a design yet.**

### Step F2-arm — Declare the arm × state matrix (substrate-native, load-bearing)

This is the step that breaks our #1 failure (§0.5). Before building, state:

- **Which arm(s)** does this run on? Direct (main thread, `peer_context` is
  `Some`) / Worker (per-peer SDK) / **both/mixed** (the normal case — a Direct
  primary with Worker backends).
- **Which state configurations** can reach it? single-peer-clean /
  many-peer / warm-boot-with-persisted-config / multi-tab / backend-hosted
  peer / foreign-cached site.
- **For each (arm × state) cell that's reachable in production, how is it
  tested?** If a cell is "tested by reading the code," say so explicitly —
  that is a known hole, not coverage. (Our Direct-arm-e2e is blind, and we
  have no durable-backend-with-site e2e — these are *standing* holes; new work
  that lands in those cells must name that it's landing there.)

A feature whose arm matrix is "Direct, single-peer" but which ships to the
Worker default is mis-scoped *by construction* — that mismatch is exactly how
every §0.5 bug shipped.

### Step F3 — Decide the D13 observability surface BEFORE writing the loop

Any operation that can run > 1 frame, or that has a degraded/fallback mode,
needs its observability decided **before** the loop, not after:

- **(a) Start log** at default level — frame-safe (a `tracing` line, not a
  panic-prone path).
- **(b) Progress** — log every N **or** a persisted field on a kernel-visible
  entity (`progress`, `last_event_at`, `status`) **or** both.
  - ⚠️ **Substrate twist:** a persisted observability field is **useless on
    the Worker arm unless the reader subscribes to its prefix**
    (`feedback_worker_cache_get_needs_subscription` — worker reads hit a cache
    mirror fed *only* for subscribed prefixes). If you add an observable field,
    the surface that reads it must `subscribe` it.
- **(c) Failure reason** — **never a silent flag flip and never a silent
  fallback.** `status=failed` is always paired with `error_reason: String`. A
  worker→Direct fallback (worker-OPFS bootstrap failed) must be a **loud,
  recoverable** state, not a silent ephemeral downgrade (the
  `NoModificationAllowedError` → silent Direct path is the live counter-example,
  handoff §2).

**Silence is the enemy (§0.5).** A loop or a fallback with no D13 channel is
shipped blind by construction — it *will* become a diffuse "it's slow / it
reset" report.

### Step F4 — Build, with the binding rules (our red-flag list)

These are the non-negotiables that have each cost us a bug. Violating one is a
self-review stop:

- **Never `Closure::forget()`** — use the `DomCtx` helpers
  (`on_window_event`/`on_action`/`listen`); they store + free on rebuild.
  (D12/AP1.)
- **Never `let _ = promise`** on a JS promise — a dropped rejecting promise
  triggers `unhandledrejection` → `location.reload()`, which **wipes the e2e
  log so "0 panics" lies** (`feedback_dropped_promise_reloads_app`). Consume
  via `spawn_local` + `JsFuture`.
- **Never decide a per-peer arm from the primary** (`as_direct().is_none()`).
  Route by the **target peer's** owning SDK; use `peers.writer_handle_for(pid)`
  / `sdk_for(pid)` (D15/AP4).
- **L0 `store()` only** for render-loop reads, boot bootstrap, or session
  scratch no peer observes. Anything observable → L1 `get/put` (D2). Every
  `store()` is a visible security opt-out.
- **Reschedule the rAF before the fallible section** / keep `frame()` panic-
  resilient — a panic there freezes the app forever (D13/AP3).
- **New tree-reading surface on the Worker arm → subscribe its prefixes** or it
  reads stale/`None` (worker cache rule, above).
- **New persisted path family** → add to `app_paths.rs`, and check cross-impl
  alignment against the SDK / sibling impls' path conventions.
- **Add → paired remove at the same change** (D9/AP5); a cleanup primitive with
  zero production callers is AP9.

### Step F5 — Test at the arm and state that matter

- **Unit test** for new computed methods / `from_entity`/`to_entity`.
- **Peer-integration test** for cross-peer contracts / new persisted schemas.
- **`make e2e-worker`** for *anything* touching peers / routing / boot / sites
  / persistence — the Worker arm is where our bugs live, and it is **mandatory
  per F2-arm**. Driving handlers directly does NOT satisfy this. The e2e must
  **exercise the feature**, not just spawn the window
  (`feedback_e2e_must_exercise_new_features`).
- **The state-configuration rule (our phase-change-N analog):** the bug
  surfaces at a *configuration*, not a count. Single-peer Direct ≠ many-peer
  mixed-arm warm-boot. If the feature touches lifecycle/persistence/routing,
  the test must reach a **multi-peer / backend-hosted / warm-boot** state, not
  a clean single-peer one. (The backend-delete bug shipped *because the e2e
  delete phase never asserted the row vanished for a backend peer*.)
- **Regression assertion is the artifact.** The test that would have caught the
  bug ships in the *same commit* as the surface.

### Step F6 — Verify through the real delivery path

"Done" is not "it compiles" and not "I ran `make wasm`." For Dom it means:

- Full `make test` green **and** `make wasm` green (native cannot catch
  cfg-gated / WASM-only errors — AGENTS.md).
- `make e2e-worker` green for the categories in F5.
- **Verify through the service worker, not just `dist/`**
  (`feedback_verify_through_service_worker`): a cache-first SW can serve
  pre-fix code even when the fix is in `dist/`. Confirm the **live bundle hash
  == fresh `dist/` hash** before claiming it ships.
- **The right WebView runtime** for runtime-sensitive work:
  Firefox-green ≠ WebKitGTK-green (`make tauri-run`; the grayscale-CSP bug,
  `2f5851f`, only showed in WebKitGTK).
- **Trace the live build** for any "frozen / timing / connect" symptom and grep
  `panicked at` *before* blaming the substrate
  (`feedback_frozen_app_is_a_frame_panic`).

If you cannot reach the surface via an e2e on the required arm, **say so
explicitly** — that is a named hole (F2-arm), not done.

### Step F7 — Close-out review (the ratchet — even when clean)

After commit, write 3–5 lines:

- **What did building this teach about the system?** ("Nothing surprising" is a
  valid answer — but look first.)
- **Any pattern with 2+ recurrences** across recent work that should become a
  helper or a discipline? (Two incidents of the same shape = promote.)
- **Refactoring surfaced but not taken?** Note it for the audit doctrine or the
  next session — don't silently drop it.
- **Did any discipline get partially-applied** or honored in letter-not-spirit?
  If so, sync the correction to the charter / AGENTS.md **this session**.

**This step is the ratchet — it is how a feature makes us *stronger*.** Skipping
F7 is exactly how "adding a feature makes us weaker" (§0.5). The disciplines
and this doc only grow if we capture what each piece of work taught us.

### Step F8 — Memory + handoff

- Non-obvious behavior shipped (a contract, a pattern, a new gate) → write a
  memory entry (one fact per file; keep the MEMORY.md index line short).
- Session ends mid-track → write a handoff at `docs/plans/HANDOFF-*.md` with
  the per-session discipline scorecard.

---

## §2 Audit Doctrine

**Input:** "Y is broken." "Something feels wrong." "X is hanging / spinning /
not showing." Or a hunch.

**Output:** root cause **known, not guessed**; fix landed; **regression gate
permanent on the arm that broke**; disciplines updated; audit doc as durable
record.

### Step A0 — Open the audit doc FIRST

`docs/plans/AUDIT-<TOPIC>-<YYYY-MM-DD>.md`. Status: OPEN. Severity. Framing in
one sentence ("hardcore foundation audit" vs "quick correctness check" — both
legitimate, but *name it*). Reserve these sections at open; each gets filled
before close:

- §0 Provenance · §1 Honest state of knowledge (CAN / CANNOT say from code) ·
  §2 Telemetry-gap map · §3 Hypotheses ranked, with falsifiers · §4 Discipline
  audit (nine questions) · §5 Candidate discipline / anti-pattern (reserved) ·
  §6 Instrumentation plan (Passes A/B/C) · §7 Data · §8 Findings · §9 Fix
  proposal · §10 Disciplines + anti-patterns adopted · §11 Process review
  (audit of the audit) · §12 Final state.

**The doc is the forcing function.** An empty section is a placeholder, not a
skip.

### Step A1 — Trace before you theorize (OUR prime — the most-violated)

This is the single rule that would have saved us the most time, so it's first.

- **The symptom is on the substrate? Add one `tracing::debug!(found=…)` at the
  suspect value and RE-RUN before forming any substrate hypothesis.**
  (`feedback_trace_values_dont_theorize_substrate`: many rounds of
  flush/subscription-race theory; one trace = `found=true` 877×; the bug was
  our logic.) The same error code means different things on different arms —
  trace, don't assume.
- **Diagnose before refactor.** If the symptom doesn't match your mental model,
  the *model* is wrong — and the cheapest fix for a wrong model is to print the
  state and look at it, not to edit a code path.
- **State-dump the (arm × state) you're actually in** *before* touching code:
  which arm is the affected peer on? Is the primary a Worker or did it silently
  fall back to Direct (handoff §2)? How many peers? Warm or clean boot?

### Step A2 — Honest state of knowledge (§1)

Two lists:

- **What we CAN say from reading the code** — specific, with `file:line`.
- **What we CANNOT say without instrumentation** — equally specific. *This list
  is the more important one. Every item is a hypothesis you have no evidence
  for.* (Borrowed framing — AP6 — is fluent prose that was never re-grounded;
  this list is the antidote.)

### Step A3 — Telemetry-gap map (§2) — the silence audit

Table every surface a user might check during the failure, and what each
*actually shows*. For us, the rows that matter are our silent-failure modes:

- Did the worker bootstrap, or silently fall back to Direct? (Is it even
  *visible* which arm we're on?)
- Is a cache `Loading`/`Pending` forever, or does it surface a timeout?
- Is a degraded mode (ephemeral, stale, offline) labeled, or silent?

If the table is mostly "empty / silent / nothing," **you have a D13 violation
regardless of what the bug turns out to be** — and fixing the silence is part
of the fix (Pass B).

### Step A4 — Hypotheses ranked, each with its cheapest falsifier (§3)

≥2 hypotheses ranked by evidence alignment. For each: symptom alignment + the
**cheapest falsifier** (usually a single log line / assertion / state-dump).
**Instrument the falsifier for EVERY hypothesis in the SAME pass** — don't only
instrument the leading one, or you pay for a second round.

### Step A5 — Discipline audit on what shipped (§4)

Run the nine review questions (charter §4) against the code *at the failing
commit*. **Name which disciplines were violated, letter vs spirit:**

- "D7 satisfied in letter" — the kernel didn't crash.
- "D7 violated in spirit" — the user-visible surface was silent / empty.

This split is where new disciplines come from. (The backend-delete bug: D9-router
in letter — a delete path exists; in spirit — it returns `Ok(false)` and the
row never leaves, *and* D10 in spirit — the e2e never asserted removal on the
arm that breaks.)

### Step A6 — Instrumentation plan, three passes, three commits (§6)

- **Pass A — Telemetry.** A log line at every hypothesis falsifier site. Zero
  happy-path behavior change. Cheap.
- **Pass B — Persisted / surfaced observability.** Add the D13 fields/surfaces
  that were missing (`error_reason`, a visible arm indicator, a fetch timeout
  that flips `Pending → Unreachable`). This is the silence the telemetry-gap
  map (A3) found — fixing it is permanent.
  - ⚠️ On the Worker arm, a new observable field must be **subscribed** by its
    reader or it reads `None` (worker cache rule).
- **Pass C — Regression gate on the arm that broke.** An `e2e-worker` phase (or
  peer-integration test) that reproduces the failure shape and asserts the fix,
  **in the (arm × state) configuration where it actually broke** — not a clean
  single-peer Direct stand-in. This becomes the permanent gate.

Each pass is its own commit, so if instrumentation perturbs the bug you can
bisect.

### Step A7 — Run, fill §7 (data) and §8 (findings)

Run the instrumented session / stress repro on the **real arm**. Capture the
trace. The data confirms one hypothesis or invalidates all of them (→ back to
A4). In §8: which hypothesis confirmed, by what evidence (`file:line`); which
dismissed and why; which disciplines violated (letter vs spirit).

### Step A8 — Fix proposal (§9), with sequencing + signoff

Each fix component named: what it changes, where it lives, cost in lines, and
**what it is NOT doing** (e.g. "not removing the barrier — the barrier is
correct; the bug is upstream"). Watch the substrate landmines explicitly — e.g.
the backend-delete fix must handle the **`sdks` Vec index-shift** that corrupts
`peer_routes` (handoff §BUG-1). **User signoff at §9 before shipping** for any
audit deeper than a quick correctness check.

### Step A9 — Discipline + anti-pattern adoption (§10)

Letter-vs-spirit gaps from §4 → discipline candidates here. Use the split:

- **Candidate** — named, defined, applied to this audit. Ratifies after a
  *second* incident proves the shape.
- **Ratified** — lands in the charter + AGENTS.md inline **this session**.

New anti-patterns: name them, give the failure mode and the fix shape (the
charter's AP catalog is the home).

### Step A10 — Process review (§11) — the audit of the audit (non-skippable)

After the fix lands, ask: *"what did we miss that allowed this?"* Four lists:

1. What the disciplines, as written, **DID** catch.
2. What they did **NOT** catch.
3. What is **STILL missing** — open candidates not yet ratified.
4. What this session's process **did right** — worth keeping.

**Without §11 the audit doc is a fix record. With §11 it is a learning record.
The learning is the point** — this is the audit-side ratchet, the twin of F7.

### Step A11 — Sync to the charter + AGENTS.md (same session)

Every discipline / anti-pattern / red-flag from §10 lands in the charter and/or
AGENTS.md inline **in the same session as the audit**. Otherwise it rots in an
audit doc no one re-reads. This is a standing close criterion.

### Step A12 — Audit-close checklist

- [ ] Data captured (§7), on the real arm.
- [ ] Cause confirmed, not guessed (§8), with `file:line`.
- [ ] Fix proposed (§9); user signoff if substantial.
- [ ] Fix landed.
- [ ] Regression gate in the suite **on the arm that broke** (e2e-worker phase
      / peer-int test).
- [ ] Verified through the real delivery path (SW hash, WebView runtime if
      relevant) — F6.
- [ ] §10 disciplines updated; charter + AGENTS.md synced (A11).
- [ ] Anti-patterns added to the catalog.
- [ ] Memory entry pinned for the failure mode + fix shape.
- [ ] §11 process review written.
- [ ] §12 final state captured.

Only after all of these does the audit close.

---

## §3 Shared spine

Both doctrines share five anchors:

1. **The charter + AGENTS.md inline is the canonical spec.** Both doctrines end
   with a sync step. If it didn't land there, it didn't land.
2. **The nine review questions.** Feature runs them in F2 (*is this design
   right?*); audit runs them in A5 (*what was missed?*). Same nine, different
   question.
3. **D13 is checked at both ends.** Feature decides the surface up front (F3);
   audit checks whether it existed (A3 telemetry-gap map). Every feature ships
   a D13 channel; every audit verifies it was usable.
4. **The arm × state matrix is the test contract.** Feature declares it (F2-arm)
   and tests in it (F5); audit reproduces the bug *in the cell where it broke*
   (A6-C). Single-peer Direct green is not coverage for a Worker-default ship.
5. **The ratchet.** Both end in a learning-extraction step (F7 / A10→A11). The
   disciplines grow via this ratchet only — no drift between what we know and
   what's written down.

### When to switch from feature → audit mid-stream

You're in F4 (build) or F6 (verify) and something doesn't add up. **Stop. Open
the audit doc (A0).** The feature pauses; the audit completes; the feature
resumes from where the audit ended, with the lessons applied. Switch on **any**
of:

- A test fails for a reason you don't understand in < 30 seconds of reading.
- The symptom doesn't match the mental model (A1 trigger).
- A "looks like" guess feels comfortable. (It isn't enough.)
- The user reports behavior you can reproduce.
- You're about to write a speculative fix without knowing the root cause.

### When to switch from audit → feature mid-stream

The audit revealed a missing surface/helper/system (e.g. "there's no fetch
timeout," "there's no visible arm indicator"). Open a feature subtask (F0–F8 in
miniature) for it. The audit pauses at A9; the feature builds the surface; the
audit resumes and verifies the fix on top of it.

---

## §4 Doctrine evolution log

| Doctrine | Change | Reason |
|---|---|---|
| Both | Initial codification | After the multi-peer boot / site-robustness audit reset. Skeleton ported from Godot `DOCTRINES.md`; substrate steps (F1.5 handoff-chain, F2-arm matrix, F3 worker-cache twist, A1 trace-before-theorize, F6 delivery-path verify) authored from our own §0.5 failure pattern. |

When a step is added/removed/modified, log it here with the reason.

---

## §5 Reading order on cold start

A new session asking "how do we work here?":

1. **`AGENTS.md`** (auto-loaded) — repo map, anti-patterns AP1–AP11, the
   binding red-flags.
2. **`DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md`** — the disciplines
   (D1–D16, the *what*) + the double-sandwich substrate model.
3. **This doc** — the doctrines (the *how* for feature work and audit work).
4. **`MODEL-BROWSER-WASM-RUNTIME.md`** — substrate ground truth
   (read first for any leak / freeze / lifetime / persistence question).
5. **The latest `HANDOFF-*.md`** — current state + the per-session discipline
   scorecard.

**Disciplines = invariants. Doctrines = procedures. Model = ground truth.
Handoff = current state.** All four are load-bearing; none substitutes for
another.

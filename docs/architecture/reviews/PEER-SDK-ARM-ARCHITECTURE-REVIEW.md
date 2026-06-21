# Peer / SDK-arm architecture review

**Trigger:** a `DeletePeer` panic in mixed mode (Direct primary +
Worker backend peers). The crash was a symptom; this review asks the
real question the user raised — *is the (peer-mode × SDK-arm ×
routing) abstraction sound, or a red flag that needs refactor?*

**Method:** three exhaustive code/doc audits (surface, callers,
docs) cross-checked by hand. Reachability claims were verified
against the actual UI, not assumed.

**Verdict in one line:** the **data model is sound**; the **API
surface is defective** (it leaks the transport arm to callers). One
live bug from it was fixed; the rest is latent but will go live the
moment any UI targets a local non-primary peer. Recommend a focused,
non-emergency refactor — not a rewrite.

---

## 1. The model as it actually is (canonical — supersedes prose docs)

The only previously-accurate description lived in code
(`src/peers.rs:1189-1204`). Stating it here as the reference:

- `Peers` = `{ sdks: Vec<Sdk>, peer_routes: HashMap<peer_id, idx>,
  primary_peer_id }`. `Sdk = Direct(PeerManager) | Worker(WorkerPeerStore)`
  (`Worker` is `cfg(wasm32)`).
- **Slot 0** is the boot/primary SDK. Its arm is decided once at
  boot: `new_direct` (native, plain `make wasm`, Tauri Linux,
  tests) or `new_worker` (browser worker boot) — and can **fall back
  Direct** if the primary's worker bootstrap fails (e.g. OPFS lock →
  `NoModificationAllowedError`).
- Each `BackendMemory` / `BackendOpfs` peer spawns its **own
  dedicated Worker SDK** appended at slot ≥1 via `attach_worker_sdk`.
  `Frontend` peers live on the boot SDK.
- Per-peer ops resolve `sdk_for(peer_id)` → `peer_routes[peer_id]`,
  **falling back to slot 0 for any unknown peer_id, silently**.
- Therefore the arm is **per-SDK, hence per-peer** — **mixed mode
  (Direct primary + Worker backends) is normal, not exotic.** The
  "the app runs in one of two arms" mental model is false.

`PeerMode` (`src/peer_mode.rs`) = `Frontend` (main thread, in-mem) /
`BackendMemory` (worker, in-mem) / `BackendOpfs` (worker, OPFS).
`PeerMode` does **not** 1:1 map to an arm: Backend* ⇒ always Worker
(own slot); Frontend ⇒ boot SDK's arm (Direct *or* Worker).

## 2. What is sound

- **`Vec<Sdk>` + per-peer `peer_routes` is the right data model** for
  hosting peers across heterogeneous runtimes. Nothing here needs a
  rewrite.
- **Router methods are arm-correct per peer.** Every `Peers` method
  that takes a `peer_id` and routes via `sdk_for` —
  `execute/query/count/dispatch_write/dispatch_remove/watch_prefix/
  observe_with_events/get_entity/tree_listing/discover_handlers_async/
  delete_peer` — picks the right arm automatically. View-model data
  paths (windows passing `&self.peer_id`) are correct **by
  construction**; the abstraction holds *where callers use router
  methods*.
- The `WriterHandle` abstraction (app-tier writers) already solves
  the arm-hiding problem correctly — proof that the right shape is
  achievable in this codebase.

## 3. The defect (three coupled leaks)

The data model is fine; **the API exposes the transport arm to
callers**. Three linked symptoms:

### 3.1 Arm-split twin APIs

`delete_peer` (sync, **panics** on Worker) vs `delete_peer_worker`
(async, **Err** on Direct). `create_new_peer` / `create_new_peer_worker`.
`set_metadata_worker` / `connect_peer_worker` (orphan worker halves,
no Direct twin). The caller must know the target peer's arm *and*
pick the right twin *and* handle sync-vs-async. This is a **missing
abstraction**, not a bug to patch. The just-shipped
`peer_host_is_worker(peer_id)` is a **band-aid**: it makes the caller
do the routing the API should encapsulate.

### 3.2 Caller-side primary-arm decisions for per-peer ops

The bug class: `if as_direct().is_none() { … } else { … }` (a
**primary-arm** query) followed by a **per-peer** op.

- **`Action::DeletePeer` (app.rs ~855)** — was live; **fixed** via
  `peer_host_is_worker`. Reference for the correct shape.
- **`handle_execute` / `handle_query` / `handle_count` (app.rs
  ~1225 / ~1154 / ~1200)** — dispatch through `primary_peer_id()`.
  **Latent, not live:** verified the Execute Console selector is
  built from `read_connected()` (connected *remote* peers) + "local"
  — it does **not** list hosted local non-primary backend peers, and
  remote selection becomes an `entity://{pid}/…` URI routed through
  the primary's pool *by design* (CLAUDE.md Execute Routing). So no
  reachable misroute today. **But** the instant any surface lets a
  user target a *local non-primary* peer (a Backend* peer's own
  tree), execute/query/count silently run against the **primary's**
  tree — a silent data-correctness bug, no panic. `query`/`count`
  additionally have **no per-peer path at all** (hard-pinned to
  primary), unlike `discover_handlers_async` which already routes
  per-peer correctly — proof the asymmetry is unprincipled.

### 3.3 Silent fallback + twin asymmetries

- `sdk_for` unknown-peer → **slot 0, silently**. This is the
  "default-to-primary" anti-pattern that already caused an earlier
  subscribe peer-scoping bug. During the window between a
  Backend* peer's worker spawn and its `attach_worker_sdk` drain,
  per-peer calls for that peer mis-route to the primary SDK.
- `create_*` twins are **primary-routed**; `delete_*` twins are
  **per-peer-routed** — same twin family, opposite routing.
- `delete_peer` prunes `peer_routes`; `delete_peer_worker` does
  **not** (stale route survives a worker-peer delete).
- No `detach_worker_sdk`: deleted backend SDK slots leak until
  reload (already in BACKLOG).
- Persistence delete is **non-transactional**: the localStorage
  keypair is removed *before* the async worker delete confirms; the
  failure path only logs (observed in the original crash log — the
  keypair was already gone when the panic fired).

## 4. Is it the right design? (the user's question)

**Yes — salvageable, not a rewrite.** The defect is localized: the
public surface should never make a caller name an arm. Target shape:

1. **Uniform per-peer surface, mirroring WorkerProxy's shape.**
   Collapse each twin into one per-peer method
   `async fn op(peer_id, …) -> Result<_, E>` where the Direct arm
   wraps its sync L0 result in a `ready()` future. Callers stop
   choosing; delete the `peer_host_is_worker` band-aid and every
   caller-side `as_direct().is_none()` branch for per-peer ops.

   **Root cause (per the entity-core-rust team's reflection, §8):**
   the twins exist because the two arms disagree on *what a peer
   reference is* — Direct (`EntitySDK { BTreeMap<_, PeerContext> }`)
   models a peer as a borrowable handle (peer_id implicit); Worker
   (`WorkerProxy`) models it as a `peer_id: String` arg, uniformly
   async. The collapse must therefore **adopt WorkerProxy's
   string-keyed flat shape as canonical** (it is already the
   uniform/async one *and* the exact surface the upstream
   `L1_WORKER_MIRRORED_SURFACE ≡ REQUEST_VARIANT_NAMES` drift-check
   enumerates — 17 ops). The cardinality bridge (Direct: resolve
   `PeerContext` by `peer_id`, then call) must live in **exactly one
   internal `match` inside `Peers`**, never caller-facing. Done this
   way, §4.1 is simultaneously the local fix *and* the consumer-side
   pre-alignment for the eventual upstream `PeerSurface` trait (§8) —
   the future trait extraction becomes mechanical, not a rewrite.
2. **Name primary-only ops honestly.** `sdk()/sdk_mut()/
   create_new_peer/load_persisted/register_backend_peer` are
   legitimately primary-bound — rename/document as `*_primary` so the
   binding is an intentional choice, not an accident a caller can
   mistake for a per-peer op.
3. **Per-peer execute/query/count.** Give them the `sdk_for(peer_id)`
   path `discover_handlers_async` already has, so a future
   local-non-primary target can't silently hit the primary.
4. **Make the misroute unrepresentable (not merely loud).** The
   entity-core-rust team's sharpening: "default-to-primary" has
   bitten twice in two layers (this; the earlier subscribe bug)
   for the same reason — it is the path of least resistance to
   *write*. A doc stating the invariant rots (see §6 — three doc
   sets rotted); a `debug_assert!` only fires in debug. The durable
   defense is to make the wrong call fail the build. Concretely:
   `sdk_for` must not return `&Sdk` with a silent slot-0 fallback —
   it returns `Result<_, UnknownPeer>`, and/or per-peer ops consume
   a `PeerRef` newtype constructable *only* via successful route
   resolution, so "op on an unrouted/defaulted peer" does not
   typecheck. Prefer a mechanism that fails compilation over a
   comment that states the rule. (`debug_assert!` + warn is the
   floor, not the goal.)
5. Twin-routing symmetry + `delete_peer_worker` route pruning +
   `detach_worker_sdk` + transactional persistence-delete ordering.

Effort: focused multi-file refactor of `src/peers.rs` + the handful
of `app.rs` caller sites. **Not an emergency** (only the delete path
was reachable and it's fixed) — but it should land before any
feature exposes local non-primary peer targeting, because at that
moment 3.2 turns from latent into a silent-corruption bug.

## 5. Why we are where we are (the trail)

Multi-SDK arrived incrementally ("Stage 2A single-SDK invariant →
2B `attach_worker_sdk` → 2C persisted backends"). The single-SDK
invariant made primary-arm == per-peer-arm *true*, so caller-side
`as_direct().is_none()` was correct **then**. Stage 2B/2C broke that
equivalence without revisiting the call sites. The twin APIs predate
multi-SDK (a Direct/Worker porting seam) and were never collapsed
once `WriterHandle` proved the better shape. None of this was
written down — see §6.

## 6. Documentation state (all stale or contradicting)

The accurate model existed **only in code** (`peers.rs:1189-1204`,
`peer_mode.rs:1-67`). Prose docs:

- `../WORKER-MODE-LIVING-DOC.md` §1 — **actively contradicts**: "the
  app runs the SDK in one of two arms"; uses `Peers::Direct`/
  `Peers::Worker` (now `Sdk::*` variants inside `Peers{Vec<Sdk>}`).
  Highest-impact false statement (it's the de-facto arm reference).
- `../specs/IMPLEMENTATION-ARCHITECTURE.md` — documents a
  `PeerManager { sdk, event_log, connected_peers, ws_listen_addr }`
  struct + `sdk.peer()` routing that **no longer exists**.
- `CLAUDE.md` "PeerManager" para — pre-`Peers` single-SDK framing.
- `../specs/PROJECT-ARCHITECTURE.md` and the peer-identity model
  notes — "wraps a single EntitySDK" / "holds one SDK" (now false);
  the former cites a nonexistent `peer_manager.rs`.
- **Stage-numbering collision:** perf "Stage A–C" vs multi-SDK
  "Stage 2A–2C" vs worker "Phase 0–3.3" — three unrelated schemes,
  no doc disambiguates them.

Corrections have been applied to CLAUDE.md, the
IMPLEMENTATION-ARCHITECTURE struct, and the WORKER-MODE-LIVING-DOC §1
header — each now states the real model and points here as canonical.
This doc is the source of truth; derive future doc edits from §1.

## 7. Recommendation

1. **Now (this session):** this review (done); doc corrections
   (done); BACKLOG refactor item (done); memory (done). The delete
   hotfix already shipped.
2. **Before any feature exposes local non-primary peer targeting:**
   execute the §4 refactor. Track as one BACKLOG item, land as a
   dedicated change with the `peer_host_is_worker` band-aid removed
   at the end (its existence is the regression signal that the
   refactor isn't done).
3. **Do not** extend the twin pattern for new ops (e.g. the deferred
   Worker-only label rename) — add them through a uniform per-peer
   method per §4.1, or the surface grows the same defect.

**Open decision for the user:** schedule the §4 refactor now, or
hold until a feature forces it? It is not urgent today, but it is the
single highest-leverage correctness investment in this layer, and
every new arm-split API makes it bigger.

## 8. Convergence with the entity-core-rust (SDK) team

The upstream team reviewed this doc against the actual SDK surfaces.
Both sides converged; the points below are folded into §4 above and
recorded here for the trail.

**Endorsed as-is, ownership unchanged.** Multi-SDK hosting policy
(which peer lives in which runtime) is genuinely app/product, not
kernel. Nothing in §4 is absorbed upstream; the refactor is ours and
correctly scoped. Do it before any feature exposes local-non-primary
targeting (their words: "do it before any feature exposes
local-non-primary targeting").

**Their sharpening of §4.4 (adopted).** Docs don't defend
invariants — §6 is literally three rotted doc sets, and the
corrections applied this session are a *bridge until §4 lands*, not
the defense. The durable fix is to make the misroute fail the build
(type-system), per the rewritten §4.4. This is the strongest single
convergence point.

**Shared root cause (their contribution, folded into §4.1).** Our
twin/band-aid defect is downstream of an SDK-side fact: the two
per-peer surfaces are un-unified on three axes — sync/async, error
type, and (the subtle one) **cardinality**: Direct = borrowable
`PeerContext` handle; Worker = `peer_id` string key. The cardinality
mismatch is *why* the twins can't be collapsed without a bridge. The
**tell** that this belongs in the type system: upstream already
hand-maintains `entity_sdk::L1_WORKER_MIRRORED_SURFACE ≡
wasm_worker_protocol::REQUEST_VARIANT_NAMES`, a compile-time
equality over a 17-op string array — "when you keep a const array of
method names asserted equal across two types, the type system is
asking for a trait and you're hand-rolling it as strings." Every
consumer that hosts both arms (us now; the Godot/Python bindings
later) independently rebuilds this unification and rediscovers the
defect (this doc's §5 cross-impl note predicted exactly that).

**The eventual upstream lift — `PeerSurface` trait.** The principled
end state is one trait over those 17 ops, in WorkerProxy's flat
`async fn op(peer_id, …) -> Result<_, E>` shape, with the Direct
impl wrapping sync L0 in `ready()` futures. **Trigger: the second
consumer** needing mixed-mode hosting (Godot or Python binding). Not
before — one consumer hand-rolling it is acceptable; lifting it for
one consumer is premature. When the trigger fires it is single-
pathway applied to the deployment-arm axis, killing the defect class
at the source. Recorded here so it is not rediscovered.

**The additive-EntitySDK head-start — decision: HOLD.** Upstream
offered to additively give `EntitySDK` the same flat, peer_id-keyed,
async op shape `WorkerProxy` has (`EntitySDK::get(peer_id, path)`
mirroring `WorkerProxy::get(peer_id, path)`), without removing
`PeerContext` — which would make our future `Box<dyn PeerSurface>`
collapse a no-op. **Converged decision: hold, don't take it now.**
Rationale: §4.1's cardinality bridge is a *single localized internal
`match` inside `Peers`*, not a caller-facing leak — so §4 is
self-sufficient and clean without the head-start. The head-start's
value is purely for the *future cross-consumer trait*, which is
gated on the same second-consumer trigger. Taking it now = upstream
infra for one consumer = the premature-abstraction both sides agree
to avoid. Revisit when the trigger fires; the contract above makes
it mechanical then.

**Net.** Model sound; fix is ours and correctly scoped; §4.1 reframed
to mirror WorkerProxy's shape (so the future lift is mechanical);
§4.4 hardened to a type-system guarantee; upstream `PeerSurface`
trait + its trigger recorded; head-start held by mutual agreement.

### 8.1 Revised sequencing after upstream's correction

Upstream realized the twin footgun is partly an SDK-side defect and
will **additively** give `EntitySDK` the flat `op(peer_id).await`
shape `WorkerProxy` already has (over the 17-op
`L1_WORKER_MIRRORED_SURFACE`; strictly additive, `PeerContext`
stays, no protocol bump). That deletes the *reason* §4.1's
hand-built cardinality bridge would exist. Revised plan — **not a
stop-all**:

| Piece | Status | Why |
|---|---|---|
| §4.4 sdk_for→Result | ✅ **LANDED & verified** | Fully ours, untouched by upstream; the foundation. Zero external caller ripple (purely internal to `peers.rs`) — confirmed independent. 164 tests / WASM / lint / e2e green. |
| §4.2 rename `*_primary` | ✅ **LANDED & verified** | Naming hygiene, independent. `primary_as_direct()` self-documents the primary binding at every call site — removes the bug's proximate cause in caller code. |
| §4.1 collapse twins | ⏸ **PAUSED — upstream-enabled** | Do NOT hand-build the Direct↔Worker bridge. Resume against the additive `EntitySDK` flat shape (collapse becomes "call the new method"). **Zero bridge code was ever written** — nothing wasted. |
| §4.3 per-peer execute/query/count | ⏸ **SHORT-HOLD** | Gets simpler against the new shape; build once. (Ops already route per-peer at the `Peers` level; the work is the app.rs caller — left untouched.) |
| §4.5 pruning/detach/transactional | ⏸ **RE-CLASSIFIED → waits for §4.1** | Upstream's message called this "independent, KEEP GOING." Traced against code: all three pieces (`persistence::delete_peer` ordering app.rs:851, `delete_peer_worker` route-prune, `detach_worker_sdk` — its only caller) live in the **DeletePeer handler** that §4.1's twin-collapse rewrites wholesale. Doing it now = guaranteed rework. Honest finding: §4.5 is delete-flow-entangled, not independent. |
| band-aid `peer_host_is_worker` | retained | Correctly signals "refactor unfinished" — and it is (§4.1 paused, not abandoned). Made `sdk_for`-Result-compatible during §4.4. Removed dead-last when §4.1 lands. |

**Checkpoint state:** the two genuinely-independent,
throwaway-proof pieces (§4.4 foundation, §4.2 hygiene) are landed
and verified. Everything else legitimately waits on the upstream
additive `EntitySDK` shape, after which §4.1 → then §4.3, §4.5,
band-aid removal all fall out cheaply and mechanically against one
shape. Nothing done is wasted; nothing remaining is blocked on *us*.

**The one open decision (for the user → upstream):** greenlight
upstream to scope + land the additive `EntitySDK` flat shape now
(their estimate: a focused day, low-risk, additive, no protocol
bump) so it is ready when §4.1 resumes. We are not idle-blocked
(4.4/4.2 done) but §4.1/4.3/4.5 cannot cleanly proceed until it
lands.

### 8.2 Upstream additive `EntitySDK` flat surface — LANDED & reviewed

entity-core-rust shipped it. **Verified against the crate, not just
the summary** (`entity-core-rust/bindings/sdk/src/sdk.rs:1397-1514`)
and traced against our actual hand-bridge sites:

- `EntitySDK::peer_or_err(peer_id) -> Result<&PeerContext, SdkError>`
  + `SdkError::UnknownPeer(String)` (404-class). This is the
  **Direct-arm twin of our §4.4 `UnknownPeer`** — both sides now
  refuse the silent default-to-primary at the type level. Pleasing
  symmetry; the bug class is dead on both arms.
- 15 flat `async fn(&self, peer_id, …) -> Result<_, SdkError>`
  methods (`get/put/put_cas/list/remove/has/execute/query/count/
  entity_count/path_count/discover_handlers/discover_types/
  inbox_list/inbox_get`), strictly additive — `PeerContext`
  untouched, our build compiles clean against it (additive
  confirmed).

**Does it clean up §4.1? Yes — concretely.** Our `Sdk::Direct(pm)`
data-path arms currently hand-bridge cardinality:
`match pm.peer_context(peer_id) { Some(ctx) => ctx.op(…), None =>
Err("no PeerContext") }` at `peers.rs` ~174 (put_and_wait), ~609/633
(execute), ~666/687 (query/count), ~710 (discover) — **~7 sites**.
Each collapses to `pm.op(peer_id, …)`: the upstream method does the
resolution + typed error internally. The handle↔string-key bridge is
**gone**; only the error-type bridge remains
(`SdkError`→`String` via `.map_err`), which is correctly ours
("native↔wire stays theirs; handle↔string-key no longer is").
Direct and Worker arms now have method-name + arg-shape parity.

**Scope is right — checked, not assumed:**
- Lifecycle twins (`create/delete/set_metadata/connect`) **not**
  in the 15 — correct: they were never a cardinality problem
  (already string-keyed on both arms); their twin-ness is
  sync-vs-async, collapsed by our uniform-async wrapping (always
  ours, never the hard part). Not blocked.
- Sync L0 render reads (`get_entity`/`tree_listing`) not replaced —
  correct: sync render-path, already string-keyed, never the bridge.
- Subscribe/Unsubscribe deferred (2 of 17) — principled and
  **non-blocking**: our observe path uses
  `ctx.store().on_prefix_change_seeded`, which already had cross-arm
  parity (`StoreAccess ↔ WorkerProxy::observe[_with_events]`) from
  earlier work. It never needed the flat L1 subscribe. Matches our
  code; deferral verified harmless.

**Verdict (initial): looked right.** Superseded by §8.3 — the
data-path half hit a future-lifetime blocker on first
implementation. Shape parity is real; *future*-lifetime parity is
not, and that is what our boundary needs.

### 8.3 §4.1 implementation blocker — empirically found

Attempting the "trivial same-named dispatch" on the first method
(`Sdk::execute` Direct arm → `pm.sdk().execute(peer_id, …)`)
**fails to compile**: `error: lifetime may not live long enough`.

**Root cause.** The upstream flat methods are `pub async fn op(&self,
peer_id, …)`. An `async fn(&self)` future **borrows `&self`** (and
`peer_id`) for its whole lifetime. Our `Sdk`/`Peers` async ops must
return `Pin<Box<dyn Future + 'static>>` — the future is *detached*
and `spawn_local`/`tokio::spawn`'d, so it cannot borrow the `&Peers`
it came from. The current code only works because
`PeerContext::execute` is `pub fn execute(&self, …) -> impl Future +
'static` (an owning future via internal Arc-clone, **not** an
`async fn`) — which is exactly why it still needs the
`pm.peer_context(peer_id)` handle lookup the collapse was meant to
delete. So shape parity (name+args) is met; **future-lifetime parity
is not**, and our detached-future boundary needs the latter.

**Scope of the blocker — the §4.1a / §4.1b split:**

- **§4.1b — data-path async collapse (BLOCKED).** The 5
  detached-future ops (`execute`, `query`, `count`,
  `put_and_wait`→`put`, `discover_handlers`) cannot collapse onto
  the flat surface while it is `async fn(&self)`. Precise upstream
  ask: expose those flat methods as
  `pub fn op(&self, peer_id, …) -> impl Future<Output=Result<_,
  SdkError>> + 'static` (resolve the peer, clone the Arc-backed
  `PeerContext` into the returned future) — **mirroring
  `PeerContext::execute` (`sdk.rs:1716/1744`, same file, already
  `pub fn` not `async fn`)**. Small, proven, additive-to-the-
  additive; no redesign. Sync-shaped flat ops (`entity_count`,
  `path_count`, `discover_*`) are fine as-is — we don't consume
  those as detached futures (our sync reads use the existing sync
  `PeerManager` surface, never the new async one).
- **§4.1a — lifecycle-twin collapse (NOT blocked, doable now).**
  `delete_peer`/`delete_peer_worker`, `create_new_peer`/`_worker`,
  `set_metadata_worker`, `connect_peer_worker` never used the flat
  surface and were never a cardinality problem — they are already
  string-keyed on both arms; their twin-ness is sync-vs-async.
  Collapse them at the `Peers` level with our own
  sync-Direct-work-then-ready-future / Worker-await wrap (disjoint
  method set from §4.1b — not throwaway if §4.1b changes). **This is
  the half that removes the `peer_host_is_worker` band-aid and
  actually closes the original delete-bug arc.**

**Revised status:** §4.1 splits. §4.1a + §4.3-for-lifecycle + §4.5
(delete-flow) + band-aid removal → **proceed now** (independent,
closes the headline bug). §4.1b → **blocked on upstream**
(`async fn` → `impl Future + 'static` on the 5 detached ops);
relay. The `PeerSurface` trait remains gated on the second
cross-consumer trigger; this lifetime fix is a prerequisite for the
additive shape to actually deliver the data-path collapse it was
shipped for.

### 8.4 §4.1b unblocked + §4.1a/b LANDED

Upstream shipped the lifetime fix exactly as specced — verified
against the crate (`sdk.rs:1422-1473`): `put/execute/query/count/
discover_handlers` are now `pub fn → impl Future + 'static`
(resolve peer sync, delegate to PeerContext's owning future,
`async move { resolved?.await }`, `SdkError::UnknownPeer` folded
in). `get/list/remove/has/put_cas` deliberately stay `async fn(&self)`
(borrow `Peer`, not `Clone`) — out of our detached scope, verified
harmless (we consume none of those as `'static` futures; our sync
reads use the existing sync `PeerManager` surface).

**§4.1b — LANDED.** The 5 detached data-path ops collapsed:
`Sdk::Direct` arms went from
`match pm.peer_context(peer_id) { Some(ctx)=>ctx.op(…), None=>Err }`
→ `pm.sdk().op(peer_id, …)` (resolve+own internally, unknown-peer →
`SdkError`). Hand cardinality bridge **gone**; only `SdkError→String`
remains (correctly ours). Verified: native 0 err, WASM ✅, 164 tests.

**§4.1a — LANDED (closes the headline arc).** `delete_peer`/
`delete_peer_worker` (+ the `Sdk` panic/Err twins) collapsed into one
uniform dual-cfg `Peers::delete_peer(&mut self, peer_id) -> Pin<Box<
dyn Future<…>>>`: Direct resolves synchronously (already-ready
future), Worker awaits the proxy. **`peer_host_is_worker` band-aid
deleted** — the done-signal; only explanatory comments + upstream's
own `proxy.delete_peer` remain. §4.5 delete-flow folded in: route
pruned on Direct-success / eagerly for Worker (peer mid-delete must
not stay routable); persistence keypair + OPFS-mark dropped **only
on confirmed delete** (transactional — fixes the original
keypair-gone-before-panic hazard). The app.rs DeletePeer handler
now makes **one uniform call, no arm decision**. Verified: native /
WASM ✅ / 164 tests / lint clean / e2e compiles; full worker e2e
run for the live delete path.

**Remaining residual (honest scope — non-defect, non-urgent, NOT
the band-aid):**
- `create_new_peer`/`_worker`, `connect_peer_worker`,
  `set_metadata_worker` twins still split. **Not the defect**: all
  primary-scoped (decided via `primary_as_direct()`, correct), no
  band-aid, never caused a bug. `set_metadata_worker` is dead code
  (`#[allow(dead_code)]`, no callers) — can simply be deleted.
  Collapsing the rest is consistency cleanup, low value / nonzero
  churn-risk; do opportunistically, not urgently.
- §4.3 (handle_execute/query/count caller passing
  `primary_peer_id()` not the selected peer) — latent (Execute
  Console doesn't expose local non-primary targets today). Simpler
  now against the flat surface; still a behavioural change to the
  console — separate, scoped, not blocking.
- `detach_worker_sdk` (dead `Sdk` slot compaction) — additive,
  BACKLOG-tracked, independent of the defect arc.

**The delete-bug arc is CLOSED:** bug → `peer_host_is_worker`
hotfix → §4.4 type-enforced routing → §4.2 naming → §4.1b/a collapse
→ band-aid deleted. The defect class is dead at the type level on
both arms (`UnknownPeer` / `SdkError::UnknownPeer`). Residual above
is genuine cleanup, not correctness debt.

### 8.5 §4.4 refinement — caught by the full worker e2e

Running `make e2e-worker` (not just compile + unit tests) caught a
real regression my first §4.4 cut introduced: a freshly
**worker-created** peer (Phase 12 "+ Frontend" in worker-boot mode)
became **invisible in the palette** — `"seen":["★ …(system)"]`,
only the primary listed. Unit tests + WASM compile did **not** catch
it; the e2e did. (Vindicates the "run the e2e on this class" call.)

**Root cause:** my first §4.4 made a `peer_routes` miss an immediate
`Err(UnknownPeer)`. But the old `unwrap_or(0)` silently covered a
*legitimate* case too: a peer created in the worker boot SDK whose
route is registered asynchronously (the spawned `create_new_peer_worker`
task has no `&mut Peers` to call `refresh_routes_for_sdk`). Post-§4.4
that peer resolved to `Err` → `has_peer_context` false → filtered
out of the selector. I conflated "unrouted in the cache" with
"unknown to the system."

**Fix (the correct §4.4 design):** `peer_routes` is a **fast-path
cache, not the authority**. `sdk_for`/`sdk_for_mut`: cache hit →
fast path; cache miss → **scan SDKs for the one that actually hosts
`peer_id`** (authoritative — covers the async-route-registration
window); only if *no* SDK hosts it → `Err(UnknownPeer)`.
`sdk_for_mut` backfills the route (self-healing cache). This
**preserves the no-silent-misroute guarantee** (a genuinely unknown
peer still errs, never slot-0/primary) while not breaking the
legitimate transient window. Strictly more correct than both the
old `unwrap_or(0)` (silent misroute) and my first cut (false
unknown). Scan runs only on a miss (rare), bounded by the small
peer count. Re-verified: native 0 err, WASM ✅, 164 tests, full
worker e2e.

Lesson logged: for changes to peer routing / arm dispatch, the
worker e2e is mandatory review — compile + unit tests structurally
cannot exercise the worker-boot async-route-registration path.

### 8.6 §4.5 "defer keypair removal" sub-item — WITHDRAWN

Two more e2e runs surfaced that my §4.5 transactional-ordering work
over-reached, in two steps:

1. I moved **`mark_opfs_for_cleanup`** into the async post-confirm
   branch. Wrong: it is a *deferred-to-next-boot cleanup queue
   marker* (the dedicated worker still holds OPFS sync handles), not
   transactional state. Restored to **synchronous on
   delete-initiation** (Phase 17 tombstone assertion).
2. I moved **`persistence::delete_peer`** (the `entity_peers`
   keypair line) into the async post-confirm branch. The Phase 17
   `entity_peers`-gone assertion then raced the worker round-trip.

**The deeper finding (not just test timing):** §4.5's "drop the
keypair only after the SDK delete confirms" was **premised on the
original *panic* mid-delete** tearing state (keypair gone + peer
stuck + crash). **§4.1a removed that panic** — the uniform
`delete_peer` has no twin and cannot panic. With the panic gone,
deferring the keypair is (a) **unnecessary** (no torn-state-on-panic
to protect against) and (b) **actively worse**: it opens a
reload-during-the-async-worker-delete window where the peer is gone
from the SDK but still in `entity_peers` → next boot **resurrects
it**. Synchronous keypair removal converges correctly to user intent
even on a rare `Err` (peer won't come back after reload; OPFS dir
already tombstoned). 

**Resolution:** the "defer keypair to post-confirm" sub-item of §4.5
is **withdrawn as moot-and-harmful post-§4.1a**. Persistence cleanup
(OPFS mark + keypair removal) is synchronous on delete-initiation
(pre-§4.5 behaviour). The genuine transactional hazard the original
crash showed was the *panic*, and **§4.1a is what actually fixed
it** — not ordering gymnastics. This is the disciplined call: don't
gold-plate a transactional guarantee the panic-removal already made
moot, especially when it adds a worse race. §4.5's surviving,
genuine pieces remain: route-prune parity (done in the uniform
`delete_peer`) and `detach_worker_sdk` (BACKLOG, additive).

### 8.7 CLOSEOUT — full worker e2e GREEN

`make e2e-worker`: **`1 passed; 0 failed`** (225 s, all phases). The
previously-failing assertions now pass: Phase 12 `np entity tree
found: true` (§4.4 scan-on-miss), Phase 17 `delete click:"clicked"`
+ `tombstones post-del:<pid>` + `phase 17 reload boot` + the
`entity_peers`-gone check (§4.5 sync-cleanup), selection-source +
connect green. Final: 164 native unit tests, WASM ✅, lint 4
pre-existing, **band-aid `peer_host_is_worker` / `delete_peer_worker`
= 0 live references** (the done-signal).

**Status of the §4 plan:**
- §4.4 type-enforced routing — ✅ landed (refined to cache+scan).
- §4.2 `*_primary` rename — ✅ landed.
- §4.1b data-path collapse (execute/query/count/put/discover) — ✅
  landed against upstream's `impl Future + 'static` shape.
- §4.1a delete-twin collapse + band-aid removal — ✅ landed; the
  **delete-bug arc is fully closed and e2e-validated**.
- §4.5 — route-prune parity ✅ in the uniform `delete_peer`;
  keypair-defer sub-item ✅ withdrawn (§8.6); `detach_worker_sdk`
  → BACKLOG (additive, independent).
- §4.3 (handle_execute/query/count caller passing
  `primary_peer_id()`) — **deliberately NOT done**: latent (Execute
  Console exposes no local non-primary target today), simpler now
  against the flat surface, a separate behavioural change. Tracked
  in BACKLOG, non-urgent, non-defect.
- §4.1 residual twins (`create_new_peer`/`_worker`,
  `connect_peer_worker`, `set_metadata_worker`) — **CLOSED
  (§4.1b residual collapse)**. `Peers::create_new_peer`
  + `Peers::connect_peer` now return uniform
  `Pin<Box<dyn Future + 'static>>`; `Sdk` no longer exposes per-arm
  versions. Direct's `create_new_peer` also seeds `PeerMetadata`
  on the SDK and spawns the per-peer event-bridge so the API is
  truly arm-uniform (`create_frontend_peer` shrank from ~50
  arm-branched lines to a single `spawn_local`). Done-signal:
  zero `_worker`-suffixed lifecycle methods on `Peers`. Footgun
  fixed along the way: `Sdk::peer_shared` returns `None` on Worker
  arm instead of panicking (matched its already-`Option` return
  type — was a latent panic exposed by the unified
  `handle_connect_peer`). `set_metadata_worker` was already
  deleted.

### 8.8 §4.1b residual collapse — LANDED

The opportunistic-cleanup §4.1 residual flagged in 8.7 closed in
this thread. Pattern follows §4.1a exactly: match the `Sdk` variant
inline in `Peers::create_new_peer` / `Peers::connect_peer`, Direct
returns a `ready()` future (synchronous work happens at call site),
Worker awaits the proxy. No `Sdk::*_worker` methods remain. Gates:
164 native + 14 peer_integration + 1 e2e_worker (~285 s, includes
the connect-flow stress), clippy clean, `make wasm` clean.

The defect class (silent default-to-primary / arm-split panic) is
**dead at the type level on both arms**. Remaining items
(§4.3 hard-coded `primary_peer_id` in `handle_execute`/query/count,
`Peers::sdks` Vec compaction pending upstream `terminate()`) are
genuine, scoped, non-urgent cleanup — not correctness debt.

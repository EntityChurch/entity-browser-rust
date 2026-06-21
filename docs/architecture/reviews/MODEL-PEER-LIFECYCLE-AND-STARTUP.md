# Peer Lifecycle, Identity & Startup — ground-truth model

**Why this doc exists.** A "deleted peers resurrect on reload"
bug (BUG-A) took a long trace to root-cause because the peer model has
**three different representations of "a peer"** that can drift, and the delete
path keyed on the wrong one. This write-up captures **how peer identity,
persistence, startup, and deletion actually work today** — not just the fix —
so the planned **IndexedDB front-end / persistent system-peer** migration is
designed against reality, not the stale mental model. Read this before touching
boot, persistence, or peer management.

Scope: **browser Worker mode** (the default + the arm where this lives). Direct
mode and Tauri differences are called out inline. Companion docs: the arm-split
canonical model `PEER-SDK-ARM-ARCHITECTURE-REVIEW.md`; the boot
reframe `SYSTEM-REFRAME-BOOT-CONFIG-SURFACES.md`; the durability
substrate `RESEARCH-BROWSER-STORAGE-SUBSTRATE.md`.

---

## 0. TL;DR — the bug and the lesson

**BUG-A root cause (CONFIRMED live by the user):** `persistence::delete_peer`
matched the **stored `peer_id` field** in the `entity_peers` localStorage line.
But every other layer identifies a peer by its **seed-derived id**
(`Keypair::from_seed(seed).peer_id()`). When those disagree — *identity drift*,
e.g. a peer-id/hash-type derivation that changed across app versions on an
accumulated profile — Delete matched nothing: it **logged success**, the row
**vanished** (registry self-heal), and the peer **resurrected on reload**
because it was never actually removed from localStorage. Fixed by matching the
**seed-derived id** (`src/persistence.rs::delete_peer`, with a loud `warn!` on a
no-op delete; `primary_peer_id_for_boot` derives too). Gated by e2e
`drifted_identity_peers_are_durably_deletable`.

**The lesson that matters for the migration:** *there is no single source of
truth for "which peers exist."* There are three sets, derived from two stores,
and they are only **eventually** consistent. Any new persistence backend must
either collapse them to one authoritative set or replicate the reconciliation
deliberately. §3 and §6 are the load-bearing sections for that work.

---

## 1. Peer identity — what a "peer id" actually is

- A peer **is** an ed25519 keypair. The 32-byte **seed** (== ed25519 secret) is
  the only thing that must be persisted; everything else derives from it.
- The **authoritative id** is `keypair.peer_id()` — a hash of the public key
  (`core/crypto/src/lib.rs`, `peer_id()` / `peer_id_with_hash_type`). It is a
  function of the seed: `from_seed(seed).peer_id()` reproduces it.
- **`from_seed(secret_key_bytes(kp))` round-trips** to the same keypair, so the
  seed is a faithful, sufficient persistence unit.
- ⚠️ **The id is NOT seed-only — it also depends on the hash type / encoding.**
  If that derivation ever changes (a different default hash byte, base
  encoding, etc.), the *same seed* yields a *different* id string. This is the
  drift that bit us: an old stored id string no longer equals the current
  `from_seed(seed).peer_id()`.

**Invariant (now enforced by the fix, must hold for any backend):** the seed is
authoritative; the id is **derived, never trusted from storage**. Any code that
reads a stored id string and compares it to a live id is a latent drift bug.

---

## 2. The persistence record (`entity_peers`, localStorage)

`src/persistence.rs` (the `wasm` module) is the **only** writer of the
`entity_peers` localStorage key — verified by grep; there is no bulk re-save
anywhere. Format is line-oriented, `|`-delimited, four fields:

```
peer_id_field | seed_hex | label | mode
```

- `mode` ∈ `frontend | backend-opfs | backend-memory` (last field; missing →
  `frontend`, so an unknown future mode never drops a peer).
- `peer_id_field` is **informational/legacy**. `load_all_peer_entries` builds
  `PersistedPeer { keypair: Keypair::from_seed(seed) }` and **ignores the
  field** — the hosted id is always re-derived. The field existed only because
  `delete_peer` and `primary_peer_id_for_boot` read it; both now derive instead.
  *Candidate cleanup for the migration: drop the field entirely, or assert
  field == derived on load and self-heal.*

Sibling keys, same module: `entity_opfs_tombstones` (peer-ids whose
`workers/{id}/` OPFS dir must be removed at next boot — see §5), and elsewhere
`entity_fast_paint`, KB seed-fingerprint keys. The **keypair lives in
localStorage; the tree/journal lives in OPFS** — two different durability
substrates with different lifetimes. That split is central to the migration.

Direct mode / Tauri WebView: same `entity_peers` localStorage record. The
difference is the *tree* store (in-memory ephemeral on Direct), not the peer
record.

---

## 3. The three "peer sets" — the crux

At runtime there are **three** representations of "the peers", and they are only
eventually consistent:

| # | Set | Backing store | Lifetime | Read by |
|---|-----|---------------|----------|---------|
| **A** | **Spawn list** — `entity_peers` lines | localStorage | durable | boot (`load_all_peer_entries`) decides what to host |
| **B** | **Hosted set** — `peers.peer_ids()` | in-memory (`WorkerPeerStore.peers` mirror / `PeerManager`) | per-session | routing, the registry self-heal filter |
| **C** | **Registry roster** — `…/peers/{id}` entities | OPFS-durable tree (system peer) | durable | the Peers window rows (`read_registry`) |

How they're meant to relate:

1. **A → B at boot.** `new_wasm_worker` reads A, `partition_entries` splits
   frontend vs backend. Frontends (primary + additional) go into the **boot
   worker** (SDK slot 0) via `InitParams`; each backend gets its **own
   dedicated worker SDK** (slot ≥1) via `respawn_persisted_backend_peer_into`.
   The main-thread **mirror** (`WorkerPeerStore.peers`, `PeerInfo{peer_id,…}`)
   is populated with the **derived** ids → that's B = `peer_ids()`.
2. **B → C continuously.** `PeerRegistry::sync(peers)` (app.rs, called once at
   construction + reactively) writes a registry entity for every id in B and
   **removes** registry entries whose id is not in B. So C is meant to track B.
3. **C, filtered by B, → rows.** `read_registry` lists the registry prefix from
   the **system peer's** tree, then `.filter(|rec| hosted.contains(rec.peer_id))`
   where `hosted = peers.peer_ids()` (set B). This is the **(B) self-heal**: a
   registry entry with no live host is a *ghost* and is hidden.

**Where drift comes from (each is a real failure mode):**

- **A vs B id mismatch (the bug we fixed).** If an A-line's stored id field ≠
  its seed-derived id, the row (from C, populated from B = derived ids) shows
  the *derived* id, Delete passes the *derived* id, but the old `delete_peer`
  searched A by the *field* → no match → A keeps the line → resurrection.
- **A ⊋ B (un-hosted persisted peer).** A backend whose worker fails to spawn
  (OPFS error, etc.) is in A but never enters B. The (B) self-heal then **hides
  its row** (no live host) — so it's persisted-but-invisible-but-respawned each
  boot. We did **not** hit this in the U7 world (per-worker OPFS roots removed
  the contention), but it remains structurally possible and is worth a guard.
- **C lag on the Worker arm.** Removing a registry entity on delete may not
  reflect into the subscription mirror immediately (the BUG-B class, fixed at
  `decode_notification`); the (B) self-heal exists precisely to mask C-lag so a
  deleted row vanishes at once regardless. See `peer_registry.rs` module docs.

**Design takeaway for IndexedDB:** today A (localStorage) is the durable spawn
list and C (OPFS tree) is the durable display roster; B is the live join. If the
system peer becomes a **persistent IndexedDB-backed peer**, decide explicitly
**which store is authoritative for "a peer exists"** and make the other a
derived projection. The current bug existed entirely in the *seam between A and
the live id*; collapsing A and C onto one IDB-backed authority would remove a
whole class of drift — but only if identity is keyed on the seed-derived id.

---

## 4. Startup sequence (Worker mode), end to end

`main.rs` → mode decision (`?worker=1` / default → Worker; OPFS-unavailable →
Direct fallback with the honest "not saved" banner) → `EntityApp::new_wasm_worker`:

1. **Drain OPFS tombstones** (`opfs_cleanup::run_at_boot`) *before* any worker
   grabs OPFS sync handles — race-free dir removal of previously-deleted
   backend-OPFS peers (§5).
2. **Load A**, `partition_entries`. **Fresh profile** (no frontends) → generate
   + persist a primary (the only `save` on the boot path).
3. **`BootClass::classify`** computed *before* any cold-boot generate, so a
   warm-durable returning identity is never treated as cold (boot reframe §2.2)
   — this is what stops re-seeding defaults over a returning user's state.
4. **Spawn backend workers in parallel** with the boot worker handshake; build
   the main-thread **peer mirror** (B) from the derived ids.
5. **Boot worker `InitParams`**: `primary_peer` + `additional_peers` (frontends)
   + `opfs_root = workers/{primary_peer_id}` (per-session OPFS root; per-worker
   for backends — the U7 fix that removed `createSyncAccessHandle` contention).
6. `build_wasm_app`: register windows (`window_registry.rs`), construct
   `PeerRegistry` and **`sync`** (A→B→C seeding), wire the xworker broker, build
   the Site overlay + its Worker-arm cache subscriptions.
7. **`boot_load`** (owned, awaited, runs *before* the rAF loop arms): durable
   `put_if_absent` of session-config / site defaults + boot-time peer-roster
   validation + boot-surface navigation. This is the step that made boot
   *sequenced* rather than a pile of fire-and-forget writes (killed the Phase-21
   clobber race). It seeds **only what's absent** — a persisted config always
   wins on warm boot.

Key property: **a peer is hosted iff it's in A and its worker spawns.** Boot
does not host anything from C; C is downstream of B. (So a stale C entry alone
cannot resurrect a peer — only an A line can. That's why the fix lives in A.)

---

## 5. The delete path, end to end

`Action::DeletePeer(id)` (from a Peers-window Delete button *or* the shell
`DeletePeer` verb) → `app.rs` handler:

1. Close any windows bound to the peer.
2. Classify backend vs frontend; Tauri-IPC backends route to the native side
   and **skip** local cleanup (Tauri owns their on-disk persistence).
3. **Browser cleanup (synchronous, before the async teardown):**
   - if backend-OPFS, `mark_opfs_for_cleanup(id)` (tombstone — the dir can't be
     removed now because the dedicated worker holds OPFS sync handles; drained
     at next boot, §4 step 1).
   - **`persistence::delete_peer(id)`** — removes the A line. **This is the line
     that was buggy** (matched the field; now matches the seed-derived id). It's
     synchronous and unconditional (except Tauri-IPC) so a reload during the
     async teardown window cannot resurrect from a stale A line.
4. **`session_config::repair_for_deleted_peer`** — if the deleted peer was a
   boot target / `home_site` owner, reset that reference (reactive self-heal;
   boot-time validation is the backstop).
5. **`Peers::delete_peer(id)`** (async future) — routes per-peer:
   - backend (its own worker SDK): tear down the **whole** `Sdk::Worker` —
     `remove_sdk(idx)` + `rebuild_routes()` (the `sdks` Vec index-shift must be
     rederived or routes corrupt). Worker thread is not terminated (no upstream
     `terminate`); OPFS handles freed at next-boot tombstone drain.
   - frontend-in-boot-worker: `WorkerPeerStore::delete_peer` → `proxy.delete_peer`
     → worker removes the additional peer; the main-thread mirror (B) retains
     out the id on success.
   - Direct arm: `PeerManager::delete_peer` (sync, panics-on-Worker — the
     arm-split footgun; never decide the arm from the *primary*).
6. The removal reflects into C via the registry subscription; the **(B)
   self-heal** hides the row immediately regardless of C-lag.

**Durability contract (the discipline this bug taught):** *a delete isn't done
until it survives a reload in a multi-peer configuration.* "The row vanished" is
set-B/set-C truth; durability is set-A truth. The e2e now gates both: create-based
single + multi-tab (`deleted_backend_peers_stay_deleted_*`) and the drift case
(`drifted_identity_peers_are_durably_deletable`).

---

## 6. Invariants & open structural risks (for the IndexedDB work)

**Invariants that must survive any persistence change:**

1. Seed is authoritative; **id is always re-derived**, never trusted from
   storage for identity comparison. (Violating this *was* BUG-A.)
2. Exactly one durable writer of the spawn list; no bulk re-save (a boot must
   never re-persist a roster it merely read).
3. Local cleanup on delete is **synchronous and unconditional** for
   browser-owned peers, ahead of any async teardown.
4. The hosted set (B) is ground truth for *routing*; the registry (C) is
   *derived display* and must be reconciled against B on read.

**Open structural risks to address (not bugs today, but fragile):**

- **A ⊋ B invisibility:** a persisted backend that fails to spawn is hidden by
  the self-heal yet keeps respawning. Consider surfacing un-hosted persisted
  peers as a distinct "failed/stopped" row so they're deletable, instead of
  hidden. (The migration may make spawn failure more or less likely.)
- **Two durability substrates with different lifetimes** (keypair in
  localStorage vs tree in OPFS) can desync: a peer whose A line is gone but
  whose `workers/{id}` OPFS dir lingers (tombstone not drained), or vice-versa.
  The tombstone dance (§5) is the current reconciliation; an IDB-backed system
  peer should fold these into one transactional unit if possible.
- **The legacy `peer_id` field** in the A record is dead weight and a drift
  trap. Drop it or validate-on-load during the migration.
- **`primary_peer_id_for_boot`** (the multi-tab Web-Lock key) now derives from
  the seed; if the IDB migration changes how the primary/system peer is chosen,
  re-audit that this key still matches the actually-hosted primary.

**Migration framing.** The stated plan is an **IndexedDB front-end + persistent
system peer**. The cleanest target: make the system peer's store the *single
durable authority* for both the spawn list (A) and the roster (C), keyed on the
**seed-derived id**, with localStorage retained only as the keypair vault (or
also migrated to IDB under the same transaction). That collapses the A/B/C drift
surface this bug lived in. Whatever the shape, re-validate §5's synchronous
delete contract and §4's "seed defaults only if absent" warm-boot rule against
the new store — those two are what keep delete durable and warm boots
non-destructive.

---

## 7. What changed this session (the fix + gates)

- `src/persistence.rs`: `delete_peer` matches the **seed-derived id** (field as
  fallback) + loud `warn!` on a no-op delete (D13 silence-is-the-enemy);
  `primary_peer_id_for_boot` derives the id from the seed.
- `tests/e2e_worker.rs`: three new Worker-arm gates —
  `deleted_backend_peers_stay_deleted_across_reload` (single-tab durability),
  `deleted_backend_peers_stay_deleted_with_second_tab_open` (multi-tab, proves
  no clobber), `drifted_identity_peers_are_durably_deletable` (the BUG-A
  regression gate; FAIL-without-fix). Plus helpers `read_persisted_peers`,
  `wipe_all_storage`, `frame_loop_alive`.
- Verified: 476 native · 17 peer-int · clippy · wasm · e2e 6/6; **user confirmed
  the fix live on their real accumulated profile** (delete → reload → gone).

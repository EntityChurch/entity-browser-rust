# Repo gotchas — the load-bearing failure modes

Every entry here was earned by a shipped bug. They are the half of this repo's
agent context that is too long to keep in `AGENTS.md` and too expensive to
rediscover: substrate behaviour, arm splits, trust-chain seams, harness traps,
and the named failure modes each of them produced.

Read `AGENTS.md` first — it carries the build verbs, the code conventions and the
boundaries. Come here when you are about to touch one of the areas below, or when
something is failing in a way that does not point at its own cause.

Entries are grouped by area and, within an area, kept in the order they were
earned. A `[AP*]`/`[D*]` tag refers to the anti-pattern catalog and disciplines in
`docs/architecture/specs/DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md`.

## Windows, DOM & rendering

- **A misbehaving window is usually ONE window panicking in its frame** — the
  rAF loop now survives it (`src/main.rs` reschedules *before* `frame()` and
  wraps it in `catch_unwind`, C1), so a frame panic *degrades* (`FRAME PANIC` /
  `FRAME STALL` logged, that rebuild skipped) rather than freezing the app —
  but the broken window stays broken. Drive the live `dist/` build and grep
  `window.__entity_browser_log` for `panicked at` before blaming the substrate
  or "timing." The usual trigger
  is the **arm-split footgun**: never call a Direct-arm-only API
  (`peer_context().store()`, `sdk()`, sync `delete_peer`) unconditionally — guard
  on the *bound* peer's `peer_context` (`Some` only on Direct). Some lifecycle
  ops are twin pairs (`delete_peer` vs `delete_peer_worker`); never decide an arm
  from the *primary* for a *per-peer* op.
- **A dropped *rejecting* JS Promise reloads the whole app** — `index.html`'s
  `unhandledrejection` guard calls `location.reload()`. Never `let _ = promise`
  on a fallible web API (clipboard, fetch); consume it via
  `spawn_local(async { let _ = JsFuture::from(p).await; })`. The reload wipes
  `window.__entity_browser_log`, so `count_panics` can report a **false** 0 — in
  the e2e, clear the log right before the action, sleep, then dump it.
- **Edit buffers live in DOM elements, not tree-backed state.** Persist only
  structural state (which view/item/mode); a per-keystroke `tree.put` triggers a
  snapshot rebuild that recreates the element and **destroys focus**. Use
  `data-field="..."` + read values via DOM query at save time, pack multi-field
  saves with `\x1f`.
- **CHANGING AN `<details>` INTO AN ATOM RATCHETED THE UI-LINT BASELINE — `components::disclosure`.**
  Three surfaces had hand-rolled the same `<details>` + inline-styled `<summary>` (the QR scanner,
  the QR display, and now the connector form's Advanced section), which is how one affordance ends
  up three shades of blue. It is **not** a replacement for `collapsible_header`: that one holds
  `open` in the model so a subscription firing mid-entry cannot collapse a form somebody is typing
  into; a `<details>` re-renders closed on every repaint, which is right for a section that is
  closed by default. `src/dom/peer_connections.rs` went **styles 7 → 2** and the baseline moved with
  it in the same commit.

## State, subscriptions & change detection

- **A RUNNING APP WAS TORN DOWN BY ITS OWN SAVE — the Apps window subscribed the prefix it is also
  the WRITER of, and a subscription does two jobs.** Shipped with the saves feature (`c7809470`,
  2026-08-19) and live in every deployment published since; reported from the outside as *"it keeps
  knocking me back to the start screen every few seconds"*. **Every step in the chain is
  individually correct**, which is why it survived: a running app persists on every move →
  `render_player`'s debounce writes `app_save_path` a second later → that path sits under
  `app_saves_prefix`, which the window watches so the **Saves panel** can read it (Worker arm: a
  read lands in the cache mirror only for a subscribed prefix) → the dirty flag rebuilds the section
  → `render_dom` calls `render_player`, which `clear_children`s and creates a **new `<iframe>`**.
  The app remounts and reloads the save it just wrote, so it reads as a refresh loop rather than as
  a teardown. Seven things:
  - **The fix is `RebuildGate` (`window_watch.rs`), and the framing is the reusable part: a
    subscription does TWO jobs — mirror the data and trigger the rebuild — and until now nothing
    could ask for the first without the second.** A gated subscription is registered
    unconditionally, so the mirror keeps filling and the Saves panel is untouched; only the
    dirty-marking is suppressed. The Apps window closes the gate for exactly as long as a player is
    mounted, which is precisely when that prefix is **write-only to this window**. Both halves are
    pinned by `a_closed_gate_suppresses_the_rebuild_but_not_the_subscription` — a "fix" that simply
    dropped the subscription passes the first assertion and silently re-breaks the panel after a
    reload.
  - **NO GATE COULD HAVE CAUGHT IT, and the reason generalizes.** Phase 2h.2 launches War and clicks
    Back immediately; Phase 2h.2s' Saves panel is *deliberately* mutually exclusive with a mounted
    player (the window's own comment says so). **Nothing anywhere let an app RUN for a second.** The
    native save tests cover list/backup/restore/bundle round-trip — the data, not the mount. **A
    failure that needs elapsed time while a surface is live cannot be caught by a suite that opens
    every surface and asserts immediately**; the e2e's spawn-loop-plus-assert shape is blind to it
    by construction, exactly as `make test` is blind to the e2e file.
  - **The gate is `a_running_app_survives_its_own_save`** (standalone, ~7s): launch War, press
    **Flip inside the sandbox** via `enter_frame`, then assert two *independent* facts — the save
    **landed** and the iframe is the **same element**. Verified red before the fix (`alive:false,
    saves:1`) and mutation-checked after (ungate the two watches → red on exactly that).
  - **THE SURVIVAL PROBE IS A JS EXPANDO AND THE SAVE COUNTER IS PAGE-LEVEL, AND BOTH ARE FORCED BY
    THE SAME FACT: THE REGRESSION DESTROYS THE OBJECT.** An attribute on the iframe — where every
    other host-side observable lives, beside `data-app-state-seq` — is wiped at exactly the moment
    the gate needs to read it back, so `window.__entity_app_save_seq` carries "did a save land" and
    an expando carries "is this the same element". **When the failure mode destroys a thing, the
    evidence about it cannot live on that thing.**
  - **`window.frames.length` reads 0 here and `document.querySelectorAll('iframe')` finds nothing** —
    every window section lives in a **shadow root**, which injected script cannot enumerate from the
    top document. WebDriver's frame enumeration is separate, and `enter_frame(0)` reaches it fine
    (measured; it is how the Flip click lands). Don't "harden" a frame-index assumption by scripting
    a frame count — it is always zero, and asserting on it fails a healthy page.
  - **Programs is NOT affected, and why is the diagnostic.** It watches only its own view-state
    path, so the saves its L5 players write (`PROGRAMS_SET`, outside `APP_SETS`) reach no watcher.
    **The bug needs one window to be both reader and writer of one prefix** — that is the shape to
    grep for, not "a window with an iframe".
  - **Residual, named and NOT closed:** a foreign **catalog/bundle** fetch landing mid-game still
    rebuilds and still tears the app down — `ensure_fetched` is one-shot per window-open, so it
    fires near launch rather than periodically, which is why it does not produce this symptom — and
    a **locale switch** re-inits every running player deliberately (`force_all`). Neither is gated.
    The content-site document frame is the same *class* (a rebuild remounts the document and drops
    its scroll and anchor history) but has no periodic writer under its watched prefixes; checked,
    not measured further.
- **Worker-arm `get_entity` / `tree_listing` read a main-thread cache mirror
  populated ONLY for subscribed prefixes** — any new tree-reading surface (window,
  overlay, app-level reader) must `WindowWatch`/`observe` exactly the prefixes it
  reads, or the write you just made is unreadable. (This is the concrete substrate
  reason behind "subscribe, don't poll.") A boot-time settings read in Worker mode
  returns the default (cache not seeded) — use an async round-trip or accept it.
  - **The e2e monolith CANNOT catch a missing app-tier subscription, and a green
    suite is not evidence that a window subscribes what it reads.** Measured
    2026-08-16 (d): every window is open in `worker_boots_and_opens_all_windows`,
    and **two of them subscribe the whole peer tree** — Entity Tree
    (`observe_with_events` on `/{pid}/`) and Storage (`watch_prefix` on the same)
    — while the proxy cache is a union (below). With those open, deleting a
    window's own `watch_prefix` changes nothing observable: the read still lands
    in somebody else's mirror. Verified by mutation, on a phase written to catch
    exactly that: it stayed green. **A subscription claim needs a boot with that
    window alone** (`a_lone_file_transfer_window_lists_what_it_offers` — 3.7s,
    asserts the precondition that no whole-tree subscriber is open, and is red
    with the subscription removed). This is AP20 wearing a test harness: the read
    works *because of another subscriber*, which is also the shape that made the
    theme dropdown permanently stale.
  - **That cache is a UNION over every subscription's mirror — reads are global,
    but invalidation is per-subscription.** `WorkerProxy::cache_list`/`cache_get`
    merge *all* subscription entries, so a path stays readable while **any one**
    mirror holds it. With N subscribers on a prefix, a removal arrives as **N
    separate `Change` events**, and each used to poke only its own
    `notify_tx` — so a reader whose own event landed before the last one woke,
    read a union the others hadn't cleaned yet, and **was never woken again**.
    Permanently stale, decided by delivery order, so it presents as flake and
    ignores polling. *Measured:* a deleted theme held by 4 subs drained
    #14→#1→#8→#17 while the reader owning #1 reconciled after its own event and
    never again (`AUDIT-THEME-DELETE-STALE-DROPDOWN-2026-08-13` §8 F3).
    **Fixed upstream** — a `Change`/`Snapshot` now wakes every subscription whose
    prefix covers the path (`entity-core-rust bindings/wasm-worker-proxy`), and
    that crate now traces mirror lifecycle in **debug builds**
    (`worker-proxy: removal … remaining_holders=[…]`; compiled out of release).
    The standing rule: **any shared read surface must be invalidated across the
    same scope it is read across** (AP20).
  - **Still true regardless: `tree_listing` is a cache, not an authoritative
    enumeration.** But the fix once recorded here — "that wants
    `tree_listing_async`" — is **WRONG, and `delete_site` must keep its sync
    listing.** Tried, measured, reverted (third session, 2026-08-13). The two
    are not the same operation: sync `tree_listing` → `cache_list` →
    `starts_with(prefix)` is **recursive**, every descendant at any depth;
    `tree_listing_async` → L1 `List` → `handle_listing` is the **immediate
    children only**. For `…/sites/{id}/` that returns the manifest and stops,
    leaving `pages/**` behind — measured on the Worker arm, `authoritative = 1`
    vs `cached = 3`, and Phase 2-SE caught it red. It converts a *hypothetical*
    orphan into a *guaranteed* one.
    Recursing by hand doesn't rescue it either: a directory child comes back as
    `hash: None` + `has_children: true`, and the worker wire type
    (`WireListingEntry` = path + content_hash) carries neither, so directory
    entries can't cross the worker boundary at all. **An authoritative recursive
    enumeration is not expressible on the Worker arm today** — closing it is an
    upstream change in `entity-wasm-worker-protocol` (bindings, ours to fix),
    not a call-site swap. The reasoning is inline at `delete_site`.
    The proxy logs `worker-proxy: removal … remaining_holders=[…]`
    per removal — read that before theorising.
    General lesson: before swapping a sync read for its `_async` twin, check
    they enumerate the **same shape**; "authoritative" and "recursive" are
    independent properties, and the names here imply neither.
- **Every render input must carry its own dirty signal.** The rule is NOT
  "reconcile before render" — that is the positional symptom. If the render reads
  an in-memory structure (a `thread_local!`/`static` registry, a cache) that can
  change *without* dirtying the surfaces reading it, that surface can go
  permanently wrong and no amount of re-polling will fix it. The theme dropdown
  reads `theme_tokens::USER_THEMES`, which `register_user_theme` /
  `unregister_user_theme` mutate while touching no `DirtyFlag`; its invalidation
  was borrowed from a *different* subscriber's watch on the same prefix, so
  whenever the render won that race the correction was never painted
  (`app.rs` — `user_themes.sync` now runs BEFORE `dom.render`, and that order is
  load-bearing). `peer_registry.sync` looks identical and is **fine** — it writes
  to the tree, and tree writes dirty their watchers; check the mechanism, not the
  line order. **`i18n`'s catalog/locale roster WAS the standing unchecked case; it is checked now
  (2026-08-23) and the answer is NOT APPLICABLE — measured, not argued.** The suspicion was
  reasonable and came from `available_locales`' own doc comment calling itself *"twin of
  `all_themes()`"*, which is exactly the shape that goes stale. The two are **not** twins:
  `all_themes()` returns a `Vec` merging the static `THEMES` with the mutable `USER_THEMES`
  thread-local, while `available_locales()` returns `&'static [Locale]` over a `const` array — there
  is **no `register_locale`/`unregister_locale`**, no user half, and overlays are embedded by
  `build.rs`, so the roster cannot change at runtime and has nothing to go stale against. The one
  thing that *does* vary is the **active** locale, and it already carries its signal:
  `LOCALE_GENERATION` is bumped by `mark_all_dirty` on every apply and consumed once per frame in
  `dom/mod.rs` as `force_all`. **The transferable half is the method, because "not applicable" is a
  real answer and only reachable by reading the mechanism:** the discriminator was never *"does a
  surface read an in-memory registry"* — every one of these does — it is **"can anything mutate it
  after boot"**. Ask that first; a `&'static` return type answers it in one line.
  - **A frame-pumped component cannot detect its own changes by diffing around
    its `pump`** (AP21). A pump *starts* async work; the results land **between**
    frames, so a before/after snapshot taken either side of the call is equal
    exactly when something changed a moment ago — the caller reports "nothing
    happened", nothing repaints, and the surface freezes on a stale state. It
    froze a running `meet` on "Searching…" at a node whose dial had already
    failed. The component owns a **change flag** set at every mutation site
    (including inside spawned landings) and the surface consumes it
    (`MeetSession::take_changed`). Note what missed it: three native tests and a
    poll-until-true loop — a loop that re-reads until it sees the change cannot
    notice that nobody was *told*. Assert the frame-shaped thing instead: on the
    frame the state settles, the pump must have said so.
  - **A delete test that the optimistic local update can satisfy is not testing
    the reconcile.** e2e Phase 26.8 passed for a session over this bug because
    `delete_theme` unregisters locally *before* dispatching the tree remove.
    Phase **26.9** is the shape that works: drive the change from the far side
    (Shell `rm` straight to the tree) so only the reconcile→render path can
    satisfy the assert. It was verified **red before the fix and green after** —
    keep it that way; a gate never seen red is not a gate.

## Peers, SDK arms & the WASM substrate

- **Async main-thread peer/store construction can't run inline in a
  DOM-event/action handler** — the handler is the sync `frame()` loop and can't
  hold `&mut peer_manager` across a `build_async().await`. Defer via a
  frame-drained pending queue (`pending_idb_peers` / `pending_sdk_attachments`):
  build off-loop in `spawn_local`, land the result, insert under `&mut self` next
  frame. (The `frontend-idb` durable this-tab peer; see
  `DESIGN-PERSISTENT-THIS-TAB-PEER.md` §14. A durable peer also owns its
  `entity-peer-{id}` IDB db — delete must tombstone it for `idb_cleanup`.)
- **A flow that keeps dispatching after its first `.await` holds a `DispatchHandle`**
  (`dispatch_handle.rs`, `Peers::dispatch_handle(pid)`) — the awaited, read-capable twin of
  `WriterHandle`. Every `Peers` L1 method already returns an *owning* future, so a single call
  spawns fine; what a spawned task cannot do is make the **second** call, because that needs a
  `&Peers` the borrow already gave up. The older answer is a frame-pumped state machine
  (`MeetSession`, `ChatDelivery`) — correct for *reactive* flows, and AP21's footgun; use the
  handle for genuinely **sequential** ones (list → read each; blob → its chunks). Its `put` goes
  through each arm's own put, not a hand-rolled `system/tree:put` execute, so the Direct
  generation bump and the Worker cache-reflection wait still happen. It does **not** replace
  `WriterHandle` — a fire-and-forget mirror write should stay fire-and-forget.
- **App-tier writers hold an `Option<WriterHandle>`** (`writer_handle.rs`) — call
  `handle.put/.remove`. Do NOT reintroduce dual `shared` + `worker_proxy` fields
  with per-call cfg branching; the Worker arm gets silently stubbed to a no-op
  that compiles clean and drops writes. (`listener_state.rs` is the one
  intentionally Direct-only writer, gated `native-ws`.)
- **Audit peer-scoping at every cross-peer call site** — `default_peer_id` /
  `let _ = peer_id` that silently falls back to primary is the hidden
  anti-pattern; thread `peer_id` explicitly to the SDK call. (The deleted
  `peer_context_or_default` helper was exactly this.) For a 1:N surface (one
  resource → N consumers), trace **both** the receive-side dispatch axis and the
  add/remove lifecycle axis before sign-off.
- **WASM has a real filesystem** (OPFS + IndexedDB) — never call something
  "WASM-incompatible" for lacking POSIX FS. Watch the wasm32 edge cases:
  crypto/hash overflow on debug builds (profile overrides), WebKit denying
  `memory.grow` under pressure (pre-allocate), prefer `BTreeMap` for
  render-iterated maps. `try_borrow_mut()` in rAF, `if let Ok` on Mutex in
  `spawn_local`.
- **The worker-protocol wire types are `#![cfg(target_arch = "wasm32")]`**
  (`entity-wasm-worker-protocol` — `InitParams`, `WireWebRtcConfig`, `Request`,
  …). They **do not exist on native**, so app-side code that must produce or
  consume them (`session_config.rs`, boot wiring) cannot name the wire type in
  natively-compiled/-tested modules, and a native unit test can't reference it.
  The pattern (bit us twice — the v11 CBOR native-port attempt, then WebRTC
  provisioning): keep a **native shadow type** (e.g. `WebRtcProvisioning`) with
  the resolution logic that `make test` covers, and a thin `*_to_wire` mapping
  behind `#[cfg(target_arch = "wasm32")]` at the `InitParams` boundary. Verify
  the wire shape itself with **`make wasm-test-protocol`** (runs the upstream
  `#[wasm_bindgen_test]`s in a container — node 22, the runner isn't in the
  image), not a native test.
- **Verify storage/Worker APIs per WebView runtime** — green in Firefox/Selenium
  ≠ works in WebKitGTK (Tauri Linux lags Apple WebKit by years; missing
  `WorkerNavigator.storage` bit us). Smoke the affected feature via `make
  tauri-run`. WebKitGTK **CSP** is a recurring trap — a Tauri-injected **nonce
  nullifies `'unsafe-inline'`** unless the directive is in
  `dangerousDisableAssetCspModification` (`tauri.conf.json`): it caused a
  grayscale-UI bug (`style-src`) AND blank sandboxed **app iframes** (`script-src`
  — apps are `srcdoc` inline-script HTML that inherit the app CSP). The browser
  deployment has NO CSP, so both were Tauri-only. Watch CSP under Tauri.
- **Worker mode (and everything worker-only, incl. the §6.5 WebRTC establisher)
  needs a SECURE-CONTEXT origin** — OPFS (`navigator.storage.getDirectory`) is
  secure-context-gated, and without it the Worker bootstrap fails and the app
  **silently falls back to Direct mode** (`main.rs:444`), so a worker-only feature
  reads as "inert/unprovisioned" when the real cause is the origin.
  `localhost`/`127.0.0.1`/`https` are secure; a plain-http hostname is **not**.
  The `make e2e-worker` suite dodges this because its host-net container uses
  `localhost:8092`. Any harness serving to a browser over a **non-localhost http
  origin** (e.g. `host.containers.internal`) must mark it trustworthy —
  Firefox: `dom.securecontext.allowlist=<host>`.
  - **BUT AN INSECURE ORIGIN DOES *NOT* COST YOU WEBRTC, AND THIS ENTRY IMPLIED IT DID FOR
    MONTHS.** The rule above is about **Worker mode**, and is correct about it. The default
    browser arm is **main-thread**, and there the establisher needs `RTCPeerConnection`, not
    OPFS. Measured 2026-08-21, Firefox 149 at `http://192.168.68.55:8099/`:
    `RTCPeerConnection` **constructs and gathers ICE** (2 host candidates, mDNS `.local`);
    `indexedDB`, `WebSocket` and `crypto.getRandomValues` all work; the app boots clean with
    0 panics. What is **actually** gone is `navigator.storage` (OPFS — opt-in, off by
    default, so it costs nothing) and **`navigator.mediaDevices`** — i.e. **the QR scanner
    is dead**, which is the ironic one: the affordance built so nobody retypes a Base58 peer
    id is absent on exactly the origin you would want it on. `crypto.subtle` is gone too; we
    do not use it. **There is no `is_secure_context` gate anywhere in `src/` outside
    `readiness.rs`** — the banner and the preflight are the only things that care.
  - **So `readiness` GRADES THE ORIGIN BY MEASURED CONSEQUENCE, never by an asserted one** —
    `WARN` when WebRTC is present anyway, `FAIL` only when it is not. It used to print
    *"browsers disable WebRTC off a secure origin"* directly above its own measured
    `OK webrtc-api`. **A diagnostic that contradicts itself on adjacent lines is worse than
    one that says less**: the reader cannot tell which line to believe, so they believe
    neither, and that is how a preflight gets routed around. Pinned by
    `no_row_fails_for_a_consequence_the_report_measured_as_fine` — a property over the
    (secure × rtc) corners, not a fixture — plus
    `an_insecure_origin_that_still_has_webrtc_warns_instead_of_failing`, both
    mutation-checked. **Chrome, Safari and the mobile browsers are UNMEASURED**; a `FAIL
    origin` on one of those is data, so write it down rather than assuming it is a bug.
  - **The generalizable lesson, and it cost a whole runbook: a PLATFORM claim written from
    the spec is a PREDICTION, and the browser is sitting right there.** `docs/RUNBOOK-TWO-MACHINES.md`
    was built on "serve from A, open on B **cannot work**" and told the operator to install
    the repo on every machine. Ten minutes of WebDriver refuted it. Same shape as this
    file's two standing warnings that a **`cfg` line** and a **constant name** are not the
    code path — here it was **MDN's "secure context required"**, which is the spec's
    aspiration and not what the engine ships. Measure the engine.
- **`~/.entity/peers/` IS THE MACHINE-WIDE IDENTITY STORE, NOT TORI'S — and we adopted all of it.**
  `GUIDE-PERSISTENCE.md` §1's layout is shared by every entity-core tool on the box: the CLI, the
  conformance harness, sibling-repo fixtures. Measured on this dev box 2026-08-21: **1514
  directories, 1140 of them `vc*` validation identities holding only a keypair**, accumulating
  ~100/day since Aug 7. `load_all_peers` admitted every directory with a loadable keypair, so Tori
  held ~1500 **foreign secret keys in memory** and offered a Start button that would `.sqlite()` a
  `store.db` **into another tool's fixture directory** under a key it never issued. Reported from
  the outside as *"why do I have fifteen hundred peers, I didn't make these"* — which reads as a
  leak in our own peer creation, and is not.
  - **The discriminator was already being WRITTEN and nobody read it.** `write_default_config` has
    stamped `managed_by = "tauri"` since v0.8.0; `read_config` did not parse it. Same shape as
    *"the knobs already existed and were simply not plumbed"* — **before assuming a distinction is
    unavailable, check whether it is only missing a reader.**
  - **`config.toml` PRESENCE IS NOT THE DISCRIMINATOR, and it is the obvious wrong fix.** 317 of
    the 1514 carry one — the CLI writes them too (a hand-made desktop identity and the `test-*`
    fixtures all have `config.toml` + `grants.toml`). Exactly 3 carried `managed_by = "tauri"`, and those 3 were
    precisely what `peer-manager.json` listed. Verified live: **1514 → 3**.
  - **Listed, never adopted.** They really are peer identities on this computer, so hiding them
    would be its own lie; `list_unmanaged_identities` is **on demand** (a keypair parse each, which
    the startup path has no reason to pay) and drops the keypair as soon as the peer-id is derived.
    **Nothing is pruned** — other tooling may still reference them.
  - **The guard is at the LOOKUP, not in each caller** — `find_managed_peer_dir_by_id` for every
    mutation (delete, config toggles), `find_peer_dir_by_id` only for the legacy migration, whose
    question is genuinely "does this identity exist anywhere on disk". Not reachable from the UI
    today; a `remove_dir_all` addressed by peer-id alone should not depend on that staying true.
  - **General rule: when a directory is shared with other tools, "what is in it" and "what is
    ours" are different questions, and enumerating is not owning.**
- **Preserve the public SDK surface** even with no current callers (`#[allow(dead_code)]`,
  ask before removing) — the SDK is a product surface for any consumer, not just this app.
- **Hosted/backend peers are LOCAL** — they live in the app's `Peers` router (`src/peers.rs`),
  each its own Direct/Worker SDK; only the Direct arm exposes a `PeerContext`
  (`peer_context(pid)` is `Some` only there). **Remote/external peers live in the connection
  pool**, reached via `entity://` URIs through the local peer's execute dispatch — never via
  the local peer registry. (Authoritative: `MODEL-PEER-LIFECYCLE-AND-STARTUP` — the three
  peer sets A spawn-list / B hosted / C registry.)
- **CORE-RUST SHIPPED A FIVE-DAY 403 ON FOREIGN-NAMESPACE WRITES AND WE WERE NOT HIT — but we had
  no native test that could have told us either way, and that is the finding.** core-rust `7c21d04`
  (2026-08-18) gave the in-process sub-dispatch path a §5.2 capability ceiling where it had run no
  capability check at all, and that ceiling began *reading* the `resources` field of §6.9's default
  per-handler self-grant — encoded as a bare `*`, which canonicalizes to `/{local}/*`, i.e. **own
  namespace only**. So from 08-18 to 08-23 a handler-dispatched write into `/{them}/…` was denied.
  Fixed by `db21e24` (`entity_capability::default_handler_self_grant`, the R-5 `/*/*` form). Six
  things:
  - **THIS IS OUR MOST IMPORTANT WRITE.** A cached foreign content site at `/{them}/sites/{S}/…`
    (`content_site::paths`, written by `discovery::warm_peer_sites` and the resolver's
    cache-on-read) and a chat delivery mirror at `/{them}/app/chat/{conv}/messages/…`
    (`views::chat::delivery`) are both V7 §1.4 Category A — *browsing somebody else's site* and
    *receiving somebody else's messages* are the product, not an edge case.
  - **WE WERE NOT HIT BECAUSE EVERY ONE OF THOSE WRITES RUNS UNDER `DispatchCeiling::PeerRoot`,
    WHICH DOES NOT CONSULT THE RESOURCE DIMENSION AT ALL** (`connection.rs`: `PeerRoot => true`).
    Direct arm: `WriterHandle::Direct` is a raw L0 `shared.tree.put`, no dispatch. Worker arm:
    `wasm-worker-host::handle_put` → `PeerContext::put` → PeerRoot. `Peers::execute` → the SDK →
    PeerRoot. And `entity_sdk`'s `follow(Payload)` `MirrorWriter::local_tree_put` builds its
    dispatch with PeerRoot explicitly. **The one thing that broke upstream is
    `FollowMode::Continuation`**, whose standing leg is materialized by a *continuation handler* —
    a handler ceiling — and `views::chat::delivery` uses `FollowOptions::payload()`, not
    `continuation()`. One option value away.
  - **The app registers exactly ONE handler of its own** (`SignalingHandler`, `connectors.rs` +
    `src-tauri/src/lib.rs`) and it declares an explicit **empty** `internal_scope`, so it never rode
    the §6.9 default either. That is why nothing here changed, was disabled, or was marked flaky in
    that window — checked, not assumed: no `#[ignore]`, retry or permissions TODO was added to
    `src/` or `tests/` between 08-18 and 08-23 (the three `#[ignore]`s in the tree are the four
    `crossimpl_go_live` gates, which need core-go's publisher, and a 2026-07-03 note about a
    removed one).
  - **THE COVERAGE GAP WAS REAL AND IS NOW CLOSED — `tests/foreign_namespace_write.rs`.** Before
    it, the only thing exercising a foreign-namespace write end to end was the **Selenium-gated
    `e2e` suite** (Phase 21b / Phase 27), which is off by default and therefore silent on a
    `make check`; `tests/peer_integration.rs` had no such case, and the `content_site` unit tests
    put into a fake store rather than through a peer. Four tests: the three PeerRoot shapes the app
    actually uses, plus the regression reproduced **and** its fix pinned at the Handler ceiling the
    app does not use — so the file also fails loudly if the path dependency is ever pointed back at
    a core-rust without `db21e24`.
  - **Mutation-checked, and the mutation is the whole argument**: swap `PeerRoot` for the pre-fix
    `Handler(wildcard_handler_grant())` in `follow_payload_mirror_write_shape_lands` and it goes
    **403** on exactly that line. That is the executed answer to *"would this have failed a week
    ago"* — it would have, had our mirror been handler-dispatched.
  - **The standing rule this earns: a defect in a path dependency is only survivable if you can
    RUN the path, and "we build fine" is not that.** Our evidence that we were unaffected is a
    ceiling trace plus four tests, not a green build — the build was green through the entire five
    days, because the crate that broke (`entity-sdk`'s continuation follow) is one we compile and
    do not call.

## Connection liveness & reachability

- **Connection LIVENESS is kernel-owned now — subscribe it, do NOT add another
  mirror.** The kernel writes `system/peer/status/{peer}` (Amendment 12): `connected`
  on handshake (`connect_and_pool`), `suspect` on a dispatch transport error
  (`demote_peer_on_transport_error`, independent of keepalive), `disconnected` on a
  keepalive miss. The app **subscribes** it via the `peer_liveness` read-model
  (`liveness_of` / `read_peer_liveness_all`, keyed by Base58 `remote_pid`); every
  display resolves through `conn_display` where the kernel state is authoritative.
  - **Any surface showing connection state calls `peer_liveness::watch_all_vantages`
    at construction — one call, and it is not optional.** The reads are the easy
    half; the subscription is the half that fails *silently*. On the Worker arm
    (the default) an unsubscribed reader hits an unseeded cache mirror and gets
    `Unknown` forever — it compiles, renders, and shows a quiet dash beside a peer
    that is visibly working; and the watch is also the **wake** signal, so without
    it a surface keeps painting `Connected` straight through a real drop until
    something unrelated dirties the frame. Neither symptom points at the missing
    line. **Every vantage, not just "ours"**: `read_peer_liveness_all` merges all
    local peers' views, so watching a subset makes the read *silently partial*,
    not merely narrow. Three windows (Peer Connections, System Overview, Chat)
    hand-rolled the identical loop before it became a call — if a fourth P2P
    surface is reading liveness by hand, that is the bug, not the style.
    **Known hole:** the "every vantage" property has no native test — a native
    `Peers` holds exactly one local peer, so multi-vantage only exists on the wasm
    arms. It is covered end-to-end instead by `e2e-webrtc-meet` §6 (two browsers
    that just exchanged messages must both read `Connected`), which is the only
    gate that exercises this subscription against a real kernel status.
  The old event-sourced `connection_health` mirror — which guessed from connect
  *attempts* and lied "Connected" through a mid-session drop
  (`AUDIT-CONNECT-PEER-FILETRANSFER-2026-07-14`, `BUGLOG-2026-07-14`) — is **deleted**
  (task #3, `HANDOFF-2026-08-10-connection-health-mirror-retired`). `connections.rs`
  remains, but as the durable identity/authz **registry** ("ever connected" +
  petname + grant), NOT a liveness signal. Rules that still bind: never write a
  fourth parallel liveness store; any liveness fact belongs in the kernel shape and
  gets subscribed. The **one** app-owned transient the kernel deliberately does not
  model — a dial in flight / a dial that gave up before ever connecting — is an
  **in-memory** marker (`dial_markers.rs`), NOT a tree entity: the status enum is a
  spec-locked 3-state (Amendment 12 ruling D rejected `connecting`/`reconnecting`),
  and a dial is local action-in-progress, meaningless across a reload — a tree store
  would just re-grow the stale mirror. **The backend reconnect is now the EXTENSION-NETWORK
  driver, not a hand-rolled loop.** The `network` feature is on (`entity-peer/network`),
  and `drain_system_backend_connect` EXECUTEs `maintain-peer` (op `maintain-peer` @
  `/{S}/system/network`) once at boot; on the first 200 it disarms and the extension
  (`entity-core-rust/extensions/network`, `NetworkHandler::handle_maintain_peer`) owns
  connect-on-drop + the `system/peer/status` writes. **Load-bearing prereq:** the
  handler's `PeerLink` is bound by `Peer::start_engines` — WITHOUT it `maintain-peer`
  returns 500 "network handler not bound." The app historically ran its own app-tier
  subscription wiring and never started the kernel engines; it does now. The reactive
  `system/peer/status` surface itself is unconditional core/peer (why the read-model
  worked even before the driver). **Worker arm:** those peers' engines run in the worker
  (separate wiring, unbuilt) — a Worker-arm `maintain-peer` 500s until wired. **Validate
  any change to this path with
  `make tauri-run`, NOT e2e** — the e2e's Phase 14 is browser→a separate Tauri listener and
  never runs the in-Tauri auto-connect drain. (Task #3 + Piece C,
  `HANDOFF-2026-08-10-connection-health-mirror-retired` §4d–§4f.) [AP12-14, D1, D8]
  - **Engines and the pool are PER-LOCAL-PEER — started by a frame sweep, not at boot.**
    Both hang off the peer's own `PeerShared` (`pm.peer_shared(pid).remote`) — S→R is a
    different connection from P→R — so `maintain-peer` for a **window-bound** peer P must
    EXECUTE on `/{P}/system/network` and **500s "not bound" until `start_engines` runs on
    P**; likewise the subscription-delivery engine S runs does nothing for chat, whose
    logs live on the window peer (`views/chat/mod.rs` binds `ChatDelivery` to
    `self.peer_id`). Local peers arrive *after* boot (a durable `frontend-idb` peer lands
    frames later, a backend peer on IPC completion), so the start is
    `EntityApp::sync_peer_engines` — a per-frame roster sweep, first thing in `frame()`
    (the backend drain right after it depends on S's engines). **Add peers, not start
    calls:** per-creation-site calls are how half the roster silently ends up without
    engines. `Peers::start_engines` returns a 3-state `EnginesStart`, not a bool —
    `Unknown` (may just not be registered *yet*) is retried, `Started`/`NotApplicable`
    (Worker arm) are settled; collapsing them re-scans every frame or skips a
    still-building peer.
    (`REVIEW-2026-08-10-piece-c-landed-next-keystone-is-engines-on-window-peers` §2.)
  - **Window-bound conversations are maintained too** (`EntityApp::sync_maintained_peers`,
    Piece C step 2): a window exposes its standing remotes via
    `WindowView::maintained_remotes` (Chat = its bound 1:1's other participants) and the
    sweep EXECUTEs `maintain-peer` on the **local** peer's `/{local}/system/network`.
    Rules learned the hard way: a **Worker-arm** local peer is skipped *silently* (no
    engines ⇒ guaranteed 500; a retry burst would tell the user a poll-delivering chat is
    "unreachable"); a **first** `maintain-peer` against an unreachable peer is a **502
    that drops the session** (the 200-with-retry-armed contract covers only re-entry from
    the backoff continuation), so never give up permanently — burst, then slow-retry; and
    the sweep must settle cheaply, because `read_connections` is a tree listing + an
    entity read per row running at 60fps. **Released on unbind** via
    `release_unbound_peers` — `release-peer` with reason **`idle`**, which drops the
    maintain machinery but LEAVES the connection up (only `shutdown` evicts + writes the
    terminal status; window-close means "stop auto-reconnecting", not "hang up"). It
    **refuses the system backend**: sessions are keyed by *remote* id, so on the
    single-local-peer desktop a chat bound to B shares B's session with the backend drain
    — releasing it would kill the backend's reconnect graph while everything still read
    `connected` (`SystemBackendConnect::identity` exists to refuse exactly that, and
    outlives the drain's disarm). A real hang-up affordance must use `shutdown`, never a
    bare evict (see below). A window that derives its target from "whoever is connected"
    (File Transfer) must NOT opt in — that is circular.
  - **The drop→reconnect proof is `a_maintained_peer_reconnects_itself_after_the_remote_disappears`**
    (`peers.rs`, `memory_transport_tests`) — two peers, B's `MemoryListener` dropped to
    unregister its endpoint, asserting the app read-model flips off `Connected`, that
    `registry.dial_count()` keeps climbing with **nothing in the app re-dialing**, and
    that both status and cross-peer traffic recover when B returns. Extend THIS when you
    touch reconnect; keep it mutation-checked (with `reconnect: false` it must fail on the
    dial-count assertion — a green test that survives removing the mechanism is worthless).
    It does NOT cover the wasm frame sweep or a real WebSocket.
  - **The release proof is its counterpart —
    `releasing_a_conversation_stops_the_retries_but_leaves_the_connection_up`**
    (same module): A maintains B, then EXECUTEs `release-peer` with reason **`idle`**,
    asserting **both** halves of the teardown decision. (a) The connection SURVIVES — the
    cross-peer fetch still reaches B and the read-model still (truthfully) says
    `connected`; if `idle` ever got treated like `shutdown` upstream, closing one chat
    would hang up a peer the user is still using elsewhere, and this catches it. (b) A
    subsequent drop draws **no dials at all** — the retry loop the reconnect proof watches
    climb is genuinely gone, not merely answered with a 200. Mutation-checked: delete the
    release EXECUTE and it becomes the reconnect proof's sequence, failing on the
    dial-count assertion (verified: dials 4 → 13). 8/8 stable, ~1.2s. Same two exclusions:
    no wasm frame sweep (`release_unbound_peers` is covered only via `release_candidates`),
    no real WebSocket.
  - **The chat poll is load-bearing — do NOT "retire the poll" as a cadence tweak.** The
    A3 finding (`views/chat/delivery.rs` module doc): demoting it made delivery
    *asymmetric*: it retries §6.5 WebRTC establishment and covers the direction the
    reactive `follow` misses. Retiring it needs bidirectional subscribe-over-WebRTC +
    robust establishment — a separate investigation.
    - **It is load-bearing a THIRD way, and this one is invisible: the poll is what keeps
      the punched NAT mapping alive.** Measured 2026-08-16 (`make e2e-webrtc-idle`): across
      90s with **zero** application messages sent, the rig's routers forwarded **~4,600 UDP
      packets per side — ~51 pkt/s each**, all of it the 5 Hz `list_remote`. That is two
      orders of magnitude above ICE consent freshness (RFC 7675, ~0.2–0.5 pkt/s), and **we
      run no §5 keepalive on the WebRTC path at all** — we have never needed one. So
      `EXTENSION-NETWORK` Amendment 14's `MUST run keepalive` is **unexercised here**, and
      the §10.3 seam gate's *survives idle* half is **not dischargeable by this peer**:
      the gate exits **2 = INCONCLUSIVE**, deliberately, because a chatty peer that runs no
      keepalive would otherwise pass a gate asking "did it survive idle?" — the §11.5.1
      *measure-don't-assume* lesson one layer along. **Whoever retires the poll inherits
      Amendment 14's keepalive MUST as a NEW obligation on a path that never had one**, and
      will meet it as idle conversations dying behind NATs months later. Routed
      2026-08-16 (c); tracked as buildout items 14/15.
    - **Its cost against an UNREACHABLE peer was measured, and the stop condition is
      now UPSTREAM — not here, and not in the poll** (`EXTENSION-SIGNALING` §13 item 6,
      answered by us 2026-08-15, remedied 2026-08-14 session). The poll re-triggers
      `establish_live` at `POLL_EVERY`/60fps = **5 Hz** (measured 5.0/s); the
      establisher runs ONE negotiation and never retries it (§7.2.1
      `caller_owns_retry`, structural — `main_thread_establish.rs`), so the repetition
      is the caller's and a node saw **~5 offer deposits/second per open conversation,
      forever** — each negotiation individually conformant (§11.5's bound is *per
      establishment*, so a peer can emit 1000 deposits while satisfying it at every
      step). **Bounded at the §10.3 ladder call site** in `entity-core-rust`
      `core/peer` — a per-peer cooldown on *consultations* beside obligation 5's
      single-flight dial gate (`RemoteState::note_establish_attempt`,
      `live_establish::ESTABLISH_FREE_CONSULTATIONS`;
      `docs/PROPOSAL-ESTABLISH-CONSULTATION-BACKOFF.md`). Measured 974 → **28**
      negotiations/side/196s, ~970 → **76** deposits, with `e2e-webrtc-meet` and
      `-chat` unchanged at **4 deposits/side** (the first consultations are free, so a
      healthy meet never reaches the ramp). Two things we got wrong and you should not
      re-derive: we told arch a backoff needed a seam signal separating "unreachable"
      from "not yet" — it does not, that distinction is only needed to *abandon* a
      peer, and a backoff never does; and charging the bound on the seam's `Err`
      **exempts cancelled consultations**, which measured 57 vs 28 (charge the ask, not
      the answer). Still binding: **don't slow the poll** (A3 regression) and **don't
      add app-tier "how many times has §6.5 failed" state** — the bound lives one layer
      down, keyed and scoped correctly; a second copy here is `connection_health` again.
      `make e2e-webrtc-nat` prints the classification every run.
  - **Teardown footgun — evict ≠ release.** Two teardown paths exist: `remote.remove`
    (in-memory pool eviction — what app `disconnect_peer`/`reconnect_peer`/the
    pre-connect evict call) writes **no** status; `release_peer` (§4.2 deliberate
    teardown) writes `disconnected(local-release)`. Today every app eviction either
    removes the registry row (invisible) or is preceded by a transport error (already
    `suspect`) or immediately reconnects — so no stale `connected` surfaces. But a new
    **"disconnect but keep remembered"** affordance (plausible in Piece D/E) that calls
    bare `remote.remove` would strand the read-model at `connected` — resurrecting the
    exact stale-`Connected` lie in the surface we made authoritative. Such a flow MUST
    route through a release path that writes `disconnected` (or the eviction primitive
    must, upstream). Idle drops are covered: keepalive is default-**on** (SDK
    `KeepaliveConfig::enabled = true`, app doesn't override), so an idle-dead link
    demotes to `disconnected` in ~60s with no dispatch.
- **REACHABILITY is kernel-owned too — a successful dial publishes a transport
  profile; do NOT re-add an app-tier address book.** **Authoritative:
  `docs/architecture/reviews/MODEL-REMOTE-PEER-FACTS.md`** — which remote-peer
  fact lives where, who owns it, and the two key spaces (app = Base58, kernel =
  identity-hash hex). Read it before adding ANY new fact about a remote peer.
  `connections.rs` is now the **petname/authz registry, not an address book**:
  it no longer stores `addr`, and `RememberedPeer::addr` resolves from the route
  via `transport_profiles::address_for`. The `addr` still decoded from an entity
  body is a dated migration read (D16 cold return), never written. Rung 2 of the dispatch
  ladder (`resolve_transport_address`) lists
  `/{local}/system/peer/transport/{remote_hex}/{profile-id}`; nothing in
  production wrote those entities, so the app carried `(remote_pid, addr)` in
  `connections.rs` and handed the address back on every `maintain-peer` — the
  same inversion as the deleted `connection_health` mirror, with `no transport
  profile for peer` after a reload as its symptom. `Peers::connect_peer` now
  publishes it (`src/transport_profiles.rs`, proof
  `a_dialed_address_survives_losing_the_pooled_connection`). Rules that bind:
  **only an explicitly dialed address is published** — a traversal (WebRTC)
  connection MUST publish none, an invariant held structurally by the publish
  living at the `connect_peer` seam, so re-derive it if you add a call site.
  The write goes through `WriterHandle` so both arms are covered by
  construction. The entity type is `tcp` whatever the URL scheme is: upstream
  decodes only `…/transport/{tcp,http}` and fails closed unless the
  `transport_type` field is `"tcp"` (D5), while the resolver returns
  `endpoint.url` **with its scheme** and the dispatcher routes on that — a
  missing `…/transport/websocket` type: the **spec** landed (`NETWORK` §6.5.2b defines the
  entity), the **rust implementation did not**, and the record here said otherwise for a day.
  `TRANSPORT_WEBSOCKET` is only the `transport_type` field's *name constant* — there is no
  `TYPE_PEER_TRANSPORT_WEBSOCKET`, no decoder, and `resolve_transport_address`
  (`core/peer/src/remote.rs`) accepts exactly `…/transport/{tcp,http}`, dropping anything else
  as *"entity_type … not a supported live profile"*; an upstream test even pins that a
  websocket-typed entity must NOT decode as tcp. **So writing the "correct" type today breaks
  rung 2.** Keep `transport_type: "tcp"` for `ws://` until `core/peer` grows the profile — and
  note how the wrong conclusion was reached both times: from a **constant name**, and from a
  **`cfg` line**, neither of which is the code path. Read the resolver, read the caller.
  **`Peers::maintain_peer` is the seam that matters** — `connect_peer` is only the
  manual Connect button and the shell verb; the backend drain and the window-bound
  sweep connect via `maintain-peer`, so a publish wired only to `connect_peer`
  misses every automatic connection (it did). Publish on the **200**, never on
  issue: an address that never connected must not become a route the ladder
  spends a dial timeout on. **Teardown travels with it** — `forget_routes_to`
  sweeps every local peer on `Action::ForgetConnection`, because a Forget that
  drops the visible row while the kernel keeps the address is the
  `connection_health` disease in new clothes.
  **Staleness is unmanaged**: a repeat connect overwrites `primary` so the last
  working address wins, but nothing prunes an address that stopped working.
  Concretely — the desktop backend advertises a **LAN** address
  (`ws://192.168.x.x:4041`), so a saved profile dies when the machine changes
  networks. Not a regression (the app fed the same stale address from
  `connections.rs` before), and note that pruning is subtler than it looks:
  a peer being *offline* is not the same as its address being *wrong*, and
  deleting the route on an offline peer removes the very thing reconnect needs.
  (`REVIEW-CONNECTIVITY-LAYER-COHERENCE-2026-08-11` §4.) [D1, D8, AP17]
  - **Worker arm: subscribe `transport_profiles::routes_prefix` before reading a
    route** — but know precisely why. A *same-session* read works with no
    subscription (the write seeds the cache mirror); it is the **post-reload**
    read that needs it, and e2e Phase 14 does NOT cover that (it passes with the
    subscription removed — verified). Named hole, not coverage.
  - **The kernel already records the address separately** — `connect_and_pool`
    writes `system/connection/{remote_identity_hash}` (§3.13) carrying
    `transport` + `address` + `status`, and `mark_connection_closed` preserves it
    ("how WAS I attached"). It is diagnostics, read-on-demand, and the ladder does
    **not** consult it — so it is not a third address book to sync, and not a
    substitute for the profile. Don't wire dispatch to it.

## Connectivity — WebRTC, rendezvous, NAT & relays

- **TWO BROWSERS CHATTED AND MOVED A FILE THROUGH A DESKTOP RENDEZVOUS (2026-08-21) — the
  first time the ACQUISITION path ran outside a rig, and it exposed six defects four of our
  gates structurally could not see.** Record:
  `STATUS-2026-08-21-two-browsers-chatted-and-moved-a-file-through-a-desktop.md`; the
  runbook §5 is a transcript now, not a plan. Zero reflectors, zero relays — a LAN needs
  neither, confirmed on real hardware. **The lessons, in the order they cost the most:**
  - **A CONNECTIVITY GATE THAT IS *GIVEN* ITS CONNECTIVITY TESTS THE TRANSPORT, NOT THE
    PRODUCT.** `e2e-webrtc-chat` supplies the node, the counterpart's peer-id AND
    `?webrtc_enable=1` by URL. `e2e-webrtc-meet` is closer and still missed the worst one,
    because adding the node *through the Shell* writes the registry row whose absence was
    the bug. **No gate anywhere loads the app the way a user does** — over HTTP from a
    desktop serving it — and three of six defects lived on exactly that path. This is the
    generalized form of "the fixture is the blind spot": here the fixture was not too
    small, it was *too helpful*.
  - **"WHICH RENDEZVOUS AM I USING" HAD THREE SOURCES AND EVERY CONSUMER ASKED THE ONE THAT
    STORES A ROW.** Provisioning resolves URL query > selected connector > build knob, and
    only the middle writes to the registry. `meet` (shell **and** the Peer Connections GUI),
    and bare `connector check`, all asked `selected_connector` — so a session provisioned by
    the served URL or by `make pair-serve` had a working establisher, an empty registry, and
    was told *"no connector selected — `connector add …`"* one command after `net` printed
    `OK rendezvous`. **Both features built to eliminate typing failed on their first real
    use.** `connectors::node_in_force` is the one answer: the row when there is one, else
    `booted_snapshot()` synthesized. **Never `booted_snapshot` via a fresh resolve** — a
    shell verb holds only `&Peers` and no URL, so a re-resolve silently skips the
    highest-precedence source. Registry-MANAGING callers keep `selected_connector` (a
    synthesized node is not a row, and marking it selected offers a Remove that removes
    nothing). **Any new "do I have a node" read owes `node_in_force`.**
  - **THE TAURI WEBVIEW LOADS `frontendDist` OVER THE CUSTOM PROTOCOL, NOT HTTP — so it sees
    no URL query, ever.** The desktop hosting the rendezvous was the one client that could
    not find it: a browser on another device met peers fine while Tori's own Shell said *"no
    signaling node"*. `EntityApp::adopt_backend_rendezvous` writes the backend's node as a
    **real connector row** — listed, selectable, removable — rather than a hidden branch in
    provisioning, which would be a fourth invisible source of the same fact. Idempotent by
    the ROW (an edited label survives a restart), and it names the reload [AP22]: `meet`
    works at once through `node_in_force`, but the establisher is boot-time, so before the
    reload a peer you meet cannot connect back. **Generally: any provisioning that arrives
    by URL does not reach the desktop, and anything that must work on both needs a second
    path.**
  - **A QUERY PARAMETER IS NOT PERCENT-DECODED UNLESS YOU DECODE IT, AND THE EXISTING TEST
    HID IT BY PASSING THE LEGAL-BUT-UNENCODED FORM.** The app-server redirect encodes the
    `ws://` address (it must, or a parser truncates at the first `:`); `query_param` returned
    it raw, so the establisher installed with the literal `ws%3A%2F%2F…` and `net` reported
    it **`OK`** — nothing validates the scheme at runtime, only `build.rs` does. **When a
    value has two legal encodings, a gate that only ever sends one proves nothing about the
    other.**
  - **CHAT IS 1:1 BY CONSTRUCTION AND THE BACKEND IS NOT A HUB.** Conversation id = hash of
    the **sorted pair**; delivery is `execute tree:get` **straight at the counterpart**;
    nothing relays. `grep -i chat src-tauri/src/*.rs` → **nothing**: the system backend has
    no chat app and can never author a message. The picker lists `read_connections()`
    regardless, so it offers the backend and "chatting with the backend" is a monologue —
    which is precisely the *"isolated chats"* an operator will report. **A picker that lists
    every connected peer is claiming they can all participate.** Fixed —
    `can_participate_in_chat` filters peers we hold a **listening address** for, because a
    browser cannot bind a socket, so a listener means a native process and no native peer
    ships a chat app. It is a heuristic about a capability the protocol cannot be asked, and
    it fails in the cheap direction on purpose (a native peer that grew chat would be hidden;
    the other way costs a person a conversation with a wall).
  - **ROOMS ARE A MISSING BUTTON, NOT AN UNBUILT HALF — and the first version of this entry
    said the opposite.** `Conversation::room(creator, others, created_at)` builds an N-member
    `closed` genesis and is tested; `ChatDelivery::new` already takes N participants; the
    §1.4 union read already unions N namespaces. **`Conversation::room` has zero callers
    outside its own tests.** What is missing is a create surface plus one decision — how a
    second member learns the `conversation_id`. The 1:1 derives a well-known id from the
    **sorted pair** at `created_at: 0`; the same trick generalizes to a sorted roster. Held
    deliberately: choosing the derivation chooses the multi-party model, which is
    `ROUTING-2026-08-21` Q4. **Check `Conversation::room` before repeating that rooms are
    unbuilt.** What genuinely does not exist is a room that survives a member being offline —
    that is `EXTENSION-RELAY` Mode S, which we still consume nowhere.
  - **Also open:** File Transfer lists a `local/files` share against a **browser** peer,
    which has none (`entity-local-files` is native-only), so a spurious handler-unavailable
    error renders beside a transfer that is working.
- **TORI SERVES THE SPA ITSELF NOW — `src-tauri/src/app_server.rs`, off by default, one live
  toggle in System Overview.** The two-machine flow used to need two commands and a second release
  build (`make tauri-run` **plus** `make pair-serve`), which is one command too many for the most
  ordinary thing anyone does with this product. Six things:
  - **It supersedes `pair-serve` for the self-hosted case and needs no build.** The process running
    the rendezvous is the one handing out the app, so it already knows its own node peer-id and
    listen address and puts them in the URL it redirects to: a browser loading the bare host is
    provisioned **before boot** — no copy, no QR, no `connector add`, no reload [AP22's positive
    form]. `pair-serve` still exists for baking a node into a bundle you ship elsewhere.
  - **The redirect target is `?webrtc_node_peer=…&webrtc_node=…`**, the **top** of
    `connectors::resolve_provisioning_quietly`'s precedence and already the WebRTC harness's
    channel. Nobody types it; the typed URL is the bare host.
  - **`Request.had_query` is what terminates the redirect, and its absence is an INFINITE LOOP.**
    The query is stripped for asset lookup, so the URL we redirect *to* has path `/` and was
    redirected again — the page never loads. **No pure test could see it**: every hop is
    individually correct. `the_server_answers_a_real_request_over_a_real_socket` caught it on its
    first run, which is the argument for the `Assets` trait — the asset source is injectable so the
    whole HTTP path is testable without standing up Tauri. It also leaves `?webrtc_enable=0` alone.
  - **A FIXTURE FILENAME YOU INVENTED CANNOT FAIL THE WAY THE REAL ARTIFACT DOES.** `cache_control`
    shipped classifying the real **29 MB** wasm as `no-store` — wasm-bindgen emits
    `…-<hash>_bg.wasm`, so the hex run is not the last segment. The unit test passed because its
    filenames were made up; only reading actual `dist/` output surfaced it. Fixtures there are now
    copied verbatim from a build, and the rule mirrors `tools/cors-serve.py`'s `HASHED_ASSET`
    deliberately — that file is the reference implementation and **two expressions of one policy
    that can disagree eventually do**. Third instance of this shape after the `data:` ceiling and
    Phase 19-doc's 5 KB document.
  - **It does NOT restart the peer**, unlike the rendezvous and port-mapping toggles beside it: an
    independent listener is neither mounted on `PeerBuilder` nor bound to this run's port. The
    button says so, because its two neighbours warn that they *do* and an unexplained difference
    between three adjacent controls reads as an oversight.
  - **Three states, not two** (`AppServerView`). Serving *with* a rendezvous is "type this URL and
    you are done"; serving *without* one hands over a working app that cannot reach anybody.
    Collapsing them promises the first and delivers the second. The node hint is a **closure read
    per request**, not captured at bind, so turning the rendezvous on afterwards reaches the next
    visitor. Port falls back to ephemeral when 8081 is taken (it was, on the first live run) and
    the caller reads the port back rather than assuming.
- **THE CONNECTOR'S PEER-ID WAS A REQUIRED TEXT FIELD FOR A VALUE THE DIAL ALREADY RETURNS.**
  `Peers::connect_peer` resolves to the id of whoever answered, and `learn_node_reflectors` dials
  every freshly-added node anyway — so adding a rendezvous meant retyping a Base58 string off
  another machine's screen for something we were about to be told, on a connection we were about to
  open. `connectors::add_connector_by_address` takes a `ConnectorDraft` (address + label; the id,
  reflectors and relay behind Advanced) and learns the key. Five things, and three of them cost
  something:
  - **THE SAME SHAPE AS "the knobs already existed and were simply not plumbed" AND "ICE was never
    blocked, it had no input box" — a third instance, so treat it as the standing question: before
    keeping a field required, ask whether an operation you ALREADY perform returns that value.**
    Here the answer had been sitting in `connect_peer`'s return type the whole time.
  - **A DISCOVERED INPUT MUST NOT BECOME AN UNAVAILABLE ONE.** The first version refused the add
    when the dial failed, which quietly meant *you can only add a rendezvous that is up this
    second* — a capability regression traded for a convenience. The peer-id has **two sources** now
    (the dial, or `expect_peer_id`) and only one has to work; a failed dial is fatal only when
    nothing supplied the key, and the refusal names the field that would have avoided it. When you
    make an input optional-because-derived, ask what happens when the derivation fails.
  - **AND THE THING THAT CAUGHT IT WAS A GATE FOR SOMETHING ELSE, VIA ITS DELIBERATELY HOSTILE
    FIXTURE.** `selecting_a_connector_says_it_needs_a_reload` adds a connector at
    `ws://127.0.0.1:65535` because it is testing the *reload notice* and wants a node that will
    never answer. Nothing about that test is about reachability, and it is the only thing that
    exercised "add a node that is down". **A rig's unreachable/empty/oversized fixture is a design
    review of any change that starts caring about that dimension** — grep the suite for the extreme
    values before assuming your new precondition is free.
  - **A REMOVED FORM FIELD BREAKS EVERY GATE THAT FILLS IT, and `make test` cannot see one of
    them.** Second instance of the rule already recorded one shape along (*"a rule that REMOVES a
    control breaks every test that clicked it"*) — that was a button, this was an input, and the
    three e2e sites that set `connector_id` fail as *"no-id-field"*, which reads like a broken
    renderer. **When you change a form's fields, grep the e2e for every `data-field` you touched,
    and remember `make test` compiles the e2e to nothing.** (The peer-id sites now drive
    `connector_expect` **inside a closed `<details>`**, which is fine and worth knowing: the
    element is in the DOM, the `input` event lands in `ctx.drafts`, and the submit-time read finds
    it — no gate needs to open the disclosure.)
  - **One helper, two surfaces.** `plan_write` is the single expression of the add rules
    (validation, normalization, preserving what the node advertised, first-node-becomes-the-
    selection); `add_connector` and `add_connector_by_address` both resolve through it, and the
    shell's `connector add` accepts BOTH argument forms. The one/two-argument discriminator is
    exact rather than a guess — `validate_node_peer_id` refuses a `/`, and no address lacks one —
    and `first_arg_is_an_address` is a free function with a test asserting *that* property, not a
    hand-listed alphabet. Getting it backwards is silent in both directions (a peer-id dialed and
    reported as *"the node is down"*; an address written as a registry key nothing can resolve).
- **THE NODE IN FORCE IS LISTED EVEN WHEN IT IS NOT A ROW — `views::peer_connections::model::
  connector_rows`, and `ConnectorSource::Session` is what marks it.** Provisioning resolves URL
  query > selected connector > build knob and only the middle writes a row, so a browser that
  arrived by the desktop's served link had a working Meet sitting directly above *"no rendezvous
  nodes yet"*. It renders first (it is the one being used), marked *from the link you opened*, and
  offers **no Use or Remove** — there is no stored row to reselect or delete, so both would be
  visible no-ops. Two things: the dedup guard is a **membership test on the rows**, not "did we
  synthesize" (the opposite bug is one node listed twice, offering Use on one line and withholding
  it on the next); and the rule is a **free function** because the interesting case is unreachable
  from `make test` — `node_in_force` synthesizes only on wasm32 and its native shadow is exactly
  `selected_connector`, so through the model the insertion never runs natively. Same native-shadow
  split as `WebRtcProvisioning` against the worker wire types.
  - **AND ITS ONE BUTTON DID NOT WORK — `connectors::resolve_node`, the fix, one commit later.**
    `Check` (window **and** shell) resolved its node out of `read_connectors` alone and answered
    *"no connector with peer-id …"* about the node the same window had just labelled **In use**.
    The refusal was not too strict; it was **looking in one place**, and it had been correct right
    up until this list grew a second source. Reported by the operator on their first real run of
    the feature. Three riders: the two arms are a **type** (`ResolvedNode::Row | ::Session`) rather
    than a bool, because they differ in what a caller may *write* — a synthesized node has nowhere
    to record what it advertised, and writing anyway would materialize a connector the user never
    added, which is the one thing `node_in_force` promises not to do; the shell's `check` was also
    writing that row under **its bound peer** instead of the system peer (a second, invisible
    registry — the bug `use`/`rm`/`ls` were already fixed for); and the gate is in
    **`make e2e-webrtc-chat`**, the only rig in the tree that provisions by URL, i.e. the only one
    that has a session row to press at all.
    **The standing rule: when a list grows a second source, grep every action that resolves a row
    out of the first one.** A per-row action written against a one-source list is a refusal waiting
    for the day the list stops having one source, and it fires at exactly the user who is using the
    feature correctly.
- **THE PAIRING PROBLEM HAS NO GOOD SIDE CHANNEL, AND DID NOT NEED ONE — `make pair-serve`.**
  Joining a rendezvous means getting a Base58 peer id onto another device, and every channel
  we offered is bad: retyping it is where a real two-machine run stalls (a typo presents as a
  *connectivity* failure), and the QR scanner needs `getUserMedia`, which an insecure origin
  denies — so **the affordance built to avoid retyping is dead on exactly the origin that
  forces retyping**. The resolution is that the machine hosting the rendezvous is *also
  serving the SPA*, so **the URL the other device must type anyway can BE the pairing**:
  bake the node at build time and a browser that merely loads the page is provisioned — no
  copy, no QR, no `connector add`, and **no reload**, because the node is present before boot
  instead of added after it [AP22's positive form]. Three things:
  - **The knobs already existed and were simply not plumbed** (`ENTITY_WEBRTC_NODE_PEER` /
    `_ADDR`, read by `webrtc_provisioning_default`). Before assuming a capability is missing,
    check whether it is only missing a *caller* — the same shape as *"ICE was never blocked,
    it had no input box"* and *"the whole pipe was built; the only thing missing was a
    source."*
  - **They had NO `cargo:rerun-if-env-changed`**, so changing the node silently reused the
    cached wasm — the "a green build can be a cached build" trap on the one knob whose
    staleness is invisible (a stale node id fails as *"nobody is at the rendezvous"*). Added.
    **An `option_env!` with no rerun-if-env-changed is a latent stale-build bug; grep for the
    pair whenever you add one.**
  - **Discovery is free and a half-config is fatal.** A backend peer is stored as
    `~/.entity/backend-peers/{peer_id}` — **the filename IS the peer id**, so no crypto and no
    running Tori is needed to find it. `build.rs` **panics** on one half without the other
    (where the home-site half-config only warns) because `resolve_webrtc_provisioning` needs
    both and yields `None` for one: a half config installs no establisher while looking
    configured, which is the silent shape the whole knob exists to remove. `pair-check`
    refuses **before** the release build and, with several identities present, lists them
    rather than guessing — picking one would serve a build pointed at a node nobody runs.
- **THE LINUX DESKTOP WEBVIEW HAS NO `RTCPeerConnection`, AND NEVER DID — measured on Debian AND
  Fedora, and it is why "they meet and nothing connects".** Full record:
  `docs/status/FINDING-2026-08-22-the-linux-desktop-webview-has-no-webrtc.md`. On a **secure**
  origin, whatever `WebKitSettings:enable-webrtc` is set to: `RTCPeerConnection` **undefined**,
  `RTCIceCandidate` **undefined**, while `MediaStream` is a function and `navigator.mediaDevices` an
  object (2.50.6 in our image, 2.50.5 on this host). Media-stream is compiled in; WebRTC is not.
  Six things:
  - **Two separate gaps, and only the small one is ours.** `enable-webrtc` IS default-false and
    `wry` never sets it (zero `webrtc` hits in `wry-0.55.1/src`) — closed by
    `src-tauri::enable_webview_webrtc`, plus `gstreamer1.0-plugins-bad`/`-nice` in the Dockerfile.
    **Both are inert today**, and are kept so nobody re-bisects which half was missing.
  - **It survived because the visible half keeps working.** `meet` is an ordinary WebSocket call to
    the node, so two devices find each other and exchange ids; every establishment then fails in
    both directions with no surface naming it. It arrived as **three separate bug reports** —
    *"that device didn't answer"* in the browser's chat, *"no transport profile for peer"* from the
    desktop's file transfer, and a File Transfer window that never lists anything.
  - **No gate could see it: every WebRTC gate we own is browser↔browser.** The Tauri phases are
    display-gated and test the *backend listener*, never the WebView's engine.
  - **The preflight was right for weeks and nobody ran it.** `readiness`'s `webrtc-api` row reports
    `FAIL` correctly on the desktop. So there is a **red boot banner** now
    (`warn_if_no_webrtc_api`): being *able* to say it is not saying it.
  - **AND THE BANNER OVERSTATED THE DAMAGE ON ITS FIRST DAY — "you can find devices but nothing
    will connect, in either direction" is FALSE, and the operator caught it on sight.** WebRTC is
    how a peer with **no address** is reached; a peer that HAS one is reached by an ordinary
    WebSocket and is untouched — Connect by address, the desktop's shared folder, and everything
    served over that connection all keep working. Corrected in EN + all 30 locales, in
    `readiness`'s `webrtc-api` remedy, and in its `tracing::error` (which now names rung 1/2 of the
    §10.3 ladder as unaffected, so a log reader does not go debugging the WebSocket path).
    **Generalize it: a diagnostic that overstates is the cry-wolf failure wearing pessimism.** The
    rule this file already carries — *grade by the measured consequence, never by an asserted one*
    — was written for the insecure-origin row and applies verbatim one row down. A banner naming
    a **cause** ("WebRTC is unavailable") must scope its **effect** to the pairs that actually
    lose, or it costs the reader the paths that are fine.
  - **THE PER-PAIR ANSWER IS A MATRIX, AND IT IS WRITTEN DOWN** —
    `docs/status/REVIEW-2026-08-22-what-works-without-webrtc-the-composition-matrix.md`. Consult it
    before describing what this product can do without WebRTC, and before estimating any of the
    finding's four options; it scopes all four. Three facts in it that cost a session to establish
    and are load-bearing elsewhere:
    - **The desktop's WebView CANNOT stand in for its backend peer** — the obvious cheap fix
      ("bind Chat/File Transfer to the reachable peer") is dead. `register_backend_peer` inserts
      **metadata only**, no `PeerContext` (`bindings/sdk/src/sdk.rs:2147`; `peer_ids()`' own
      comment says *"backend peers only in metadata"*). The app can name and route to the backend
      as a **remote**; it cannot *act as* it. Acting as it is a third SDK arm beside Direct and
      Worker.
    - **§6.11(b) INBOUND REENTRY MEANS A PEER THAT HAS DIALED IS DISPATCHABLE-AT.** Every accepted
      connection registers a bidirectional reentry endpoint keyed by the dialer
      (`core/peer/src/connection.rs:185`), a dispatch that misses transport resolution falls back
      to it (`remote.rs:990`), and the **dialer's own reader task serves inbound EXECUTEs and
      writes the response back over the same write half** (`remote.rs:240`) — on wasm too. So a
      browser is reachable *by anyone it has dialed*, with no WebRTC. That is what makes "the
      native host as a relay exchanger" plausible rather than speculative.
    - **…but the reciprocal AUTHORITY is minted only for a rendezvous-key establishment**
      (`remote.rs:2062`, gated on `established_via_rendezvous_key`). A plain `ws://` address dial
      mints none, so a backend→browser reentry dispatch works under `debug_open_grants` and
      **403s under `ENTITY_BROWSER_ENFORCE`** — the same open-posture-only caveat as offers
      (buildout 22). Measure this first if anyone builds the relay.
    - **The relay is UNCONSUMED, not merely unbuilt**: the `relay` feature is off in both
      `Cargo.toml`s and `system/relay|entity_relay|system/route` has **zero** hits in `src/` and
      `src-tauri/src/`. Upstream `extensions/relay` (forward · put · poll · advertise, no
      `cfg(target_arch)`) builds for wasm today.
  - **PEER-TO-PEER SAVE TRANSFER IS BUILT AND WORKS BROWSER↔BROWSER — AND HAS NO GATE AT ALL.**
    `send_save` → `scan_peer_saves` → `import_save` (`views/games/mod.rs`) is the "you are
    connected to someone, you pull their saves in" flow: a `SaveBundle` published as an ordinary
    offer (pull, never push), candidates identified by **decoding** rather than by filename, and
    the incoming save backing up whatever it replaces. It rides offer/pull, so it works wherever
    that does. **The first version of this entry folded it into a file-transfer note and a
    matrix row marked saves "local", which read as a claim the flow does not exist — it does,
    and it is one of the more important ones.** What is genuinely missing is coverage: `make
    test` covers only the LOCAL half (list/backup/restore/bundle round-trip), the two cross-peer
    halves are `#[cfg(target_arch = "wasm32")]` so a native test cannot reach them even in
    principle, and the e2e asserts only that the Saves panel *opens*. `make e2e-webrtc-file`
    already stands up two browsers that meet and move a file — a save phase is a variation on an
    existing rig, and is the cheapest way to turn "built" into "known to work".
  - **THE DESKTOP LEG OF THE SAVE HANDOFF IS THE BLOCKED ONE, and it does not need a relay —
    the shared folder already crosses that gap.** offer/pull is dead both directions
    browser↔WebView (each end would have to dispatch at an unreachable peer), but both parties
    can read and write the **backend's** share over plain WebSocket — the browser by dialing it
    (Phase 14b), the WebView because it already dials its own backend. `SaveBundle::to_bytes`/
    `from_bytes` and `import_save` already exist; what is missing is a second **sink** and a
    second **source** for them (`local/files:write` / a share read, or a plain download / file
    picker) beside the offer. **Before proposing new transport for a cross-device story, check
    whether the bytes can ride the share** — it is the one pipe both peer forms can reach
    without WebRTC.
  - **PROBE ON THE ORIGIN THE APP USES.** The first probe used `http://probe.local/` and showed
    `mediaDevices` undefined *as well*, which pointed at a much wider (and wrong) conclusion —
    WebKit gates it on a secure context and the Tauri WebView's origin is secure.
  - **What still works, precisely**: browser↔browser through a desktop's rendezvous (chat + files),
    and browser → the desktop's **share** over its WebSocket listener (Phase 14b, no WebRTC at all).
    What cannot work is anything reaching the desktop's **WebView** peer — which is the peer its
    Chat and File Transfer windows bind to, and the id it hands out at a meet. The desktop has one
    peer that is reachable (the backend, no chat app) and one that runs the app (the WebView, not
    reachable). **Windows/macOS are a different engine and are UNMEASURED — do not claim they are
    fine.**
- **A provisioned signaling node IS the install decision — do NOT re-add a second
  enable knob.** `webrtc_install_primary(provisioned, url_override)`
  (`session_config.rs`) is the single expression both boot sites use, so the two
  arms cannot disagree about whether this session speaks WebRTC. It used to be an
  independent axis (`ENTITY_WEBRTC_ENABLE_PRIMARY` / `?webrtc_enable=1`) that had
  to be set *as well* — correct while both halves came from the deployment, and
  **wrong the moment the connector registry made the node user-selectable**: there
  was no surface for the second half, so every shipped build resolved a node and
  installed nothing, and `meet` handed back peer-ids that could never be connected
  to. What survives is the load-bearing half — **fail closed** (no node, no
  establisher) and **primary-only** (an additional peer must ask explicitly);
  `?webrtc_enable=0` stays as a kill switch for isolating whether a failure is
  WebRTC's. Generally: when a capability becomes user-supplied, ask which surface
  performs each half of the gate and whether one person can reach both. [AP22]
- **The §6.5 establisher is PRIMARY-ONLY, but `meet`/Chat act as the BOUND peer —
  check `peers.peer_has_webrtc(pid)` before handing out that peer's id.** Only the
  primary installs an establisher (an additional peer must ask explicitly — the v6
  lesson, still deliberate); meanwhile Peer Connections is `WindowScope::Peer`, the
  shell `meet` dispatches from the shell's bound peer, and `ChatDelivery` binds
  `self.peer_id`. On a single-peer cold boot these coincide, which is why nothing
  caught it. With a second local peer, a meet from it **succeeds completely** —
  discovery is an ordinary WebSocket call to the node — and the counterpart walks
  away with an id that has no way to reach back. Discovery and reachability are
  independent, and only one of them fails loudly. `Peers::peer_has_webrtc` answers
  it on both arms (Worker: `WireCaps.webrtc_peers`, the v12 install report; Direct:
  the peer we actually handed a seam to). Both meet surfaces warn on it today —
  **warn, not refuse**: the meet is legitimate (you may be introducing two other
  peers), it is the silence that was the bug. Proof:
  `a_meet_from_a_peer_with_no_establisher_warns_and_otherwise_does_not`
  (mutation-checked both ways — a guard that always fires fails it too). Any NEW
  surface that hands out a peer id, or depends on a stranger reaching it, owes the
  same check. [AP22]
- **THE PRECONDITIONS WERE ALL DOCUMENTED AND NONE OF THEM SAID ANYTHING ON THE MACHINE WHERE THEY
  FAILED — `src/readiness.rs` + the Shell's `net` verb + `docs/RUNBOOK-TWO-MACHINES.md`.** Every way
  a cross-machine connection silently does not happen was already in this file: an insecure origin
  disabling WebRTC and OPFS, a connector added but never reloaded [AP22], a meet from a non-primary
  peer, zero reflectors being a LAN-only posture. All prose, all correct, and the person who meets
  them is at a **second machine** with no harness, no `podman ps` and no stderr. `net` is one report
  — origin · webrtc-api · rendezvous · establisher · node-link · reflectors · relay · this-peer —
  each row carrying the *next action*, in fixed-width text you paste into a chat window. Five things:
  - **The judging is pure and the collection is thin.** `assess(&Facts) -> Report` is native-tested
    (the browser fields are `Option`, `None` = *not measurable here*, never `false`); `collect` is
    the wasm mapping. Same native-shadow split as `WebRtcProvisioning` vs the worker wire types.
  - **A test enforces that every non-`Ok` row has a remedy**, across the corner cases rather than one
    fixture — and it went red on its first run, on the native `webrtc-api` arm. A diagnostic's
    failure mode is silently becoming a list of nouns; that is a property, so it gets a gate.
  - **`isSecureContext` had ZERO occurrences in `src/` before this.** The rule *"worker mode needs a
    secure-context origin, and the app silently falls back to Direct"* had lived here for months as
    an instruction to agents, with nothing in the product checking it. It is a red boot banner now
    (`readiness::warn_if_insecure_origin`) and the first `FAIL` in `net`. **Generalize it: a
    precondition that fails silently on a user's machine owes a SURFACE, not an AGENTS entry — and
    an entry with no code behind it is the same theater as an invariant with no enforcement point.**
  - **The e2e phase is the only thing that proves `collect` runs in a browser at all** (Phase
    `2-net`), and it asserts the **FAIL** arm as well as the OK ones: with no connector the
    rendezvous row must fail *and* carry `connector add`. A report that is green by construction
    proves only that it prints. Remember `make test` compiles the e2e to **nothing**.
  - **The Shell had to start watching liveness** (`peer_liveness::watch_all_vantages` in its
    `create`) — the `node-link` row reads the kernel read-model, and on the Worker arm an
    unsubscribed read is `Unknown` forever. Fourth surface to need that call; it is not optional.
- **ADDING THE FIRST CONNECTOR IS CHOOSING IT — `add_connector` writes the selection when there is
  none, and the discriminator is the selection MARKER, not the resolved connector.** A registry
  holding rows with no selection resolves to **no provisioning at all** (`provisioning_from_registry`
  starts at `selected_connector`), so "I added my node" left an app that looked configured, installed
  no establisher, and handed out ids nobody could reach — [AP22] arriving through the registry
  instead of through the install. Both surfaces had the hole, so the fix is in the one function they
  share rather than in each of them. Three riders: it **never** overrides an existing choice; a
  **dangling** selection (its node removed) is still an expressed choice, so the predicate is
  `selection_marker().is_some()` and not `selected_connector().is_some()` — the first version used
  the latter and `a_dangling_selection_resolves_to_none_not_to_another_node` caught it, which is the
  §2.2 silent-substitution the module refuses everywhere else; and every write here is
  **dispatched**, so back-to-back adds read each other's pre-image (a test that does not settle
  between them is testing the race, not the rule). Gate:
  `the_first_connector_added_becomes_the_selection_and_later_ones_do_not`, mutation-checked. The
  shell's `add` now also names the reload, because the node is read once at boot [AP25].
  **A rule that REMOVES a control breaks every test that clicked it, and only the e2e finds them.**
  A selected row renders no `Use` button, so two e2e sites that did *Add → click Use* went from
  passing to waiting forever for a control that should no longer exist — e2e Phase 14.6 and
  `selecting_a_connector_says_it_needs_a_reload`. `make test` stayed green through all of it, and
  the first failure looked like a hang rather than a rule change. Both are stronger now (14.6
  asserts the row goes *In use* with nobody pressing anything; the standalone drives `Use` on a
  **second** connector, where the button COUNT — exactly one of two rows — is itself the proof the
  first add took the selection). When you change what a surface does on an action, grep the e2e for
  the control that action used to require.
- **THE PAIRING LINE IS A COMMAND, NOT TWO FACTS — System Overview's `Pair (same network)` /
  `Pair (internet)` rows.** Everything needed to join a rendezvous was already on that card: the
  node's peer id in the identity row, its address in the rendezvous row, the forwarded address in
  the port-mapping row. Each correct, and the **composition** left to a person who must know that
  `connector add` wants exactly those two in that order out of three different rows, and retype a
  Base58 peer id across a room. That is where a real two-machine run stalls, and it is a
  transcription error that presents as a connectivity one. The row renders the literal line the
  `connector add` verb parses (`// i18n-ignore` — shell syntax, not prose: translating it produces a
  command that does not run). **Both addresses, never silently one**: LAN first because a forwarded
  address is not guaranteed to hairpin back onto its own network, and the WAN row appears only when
  `external_addr` is `Some`, which is exactly *a door is open* and never *asked* or *refused*.
  **The composition is in the model, not the renderer** (`views::system_overview::output::
  pairing_commands`), because what can be wrong is the argument ORDER and whether a row appears —
  neither observable from a native test while it sits inside `create_element` calls, and this whole
  card is desktop-only behind the display gate, so no e2e phase reads it either. The gate asserts
  the **whole string** rather than `contains`: a swapped pair produces `no connector with peer-id
  ws://…` on somebody *else's* machine, which is the worst place to discover it.
- **THE APP OBSERVES ICE CANDIDATE TYPES NOW — `src/reachability.rs`; the entry saying nothing did
  is CLOSED.** The gathered types **are** the topology, so *"no reflector configured"*, *"the
  reflector is down"*, *"both sides are restrictive"* and *"your friend closed their laptop"* stop
  arriving as one sentence. Design of record:
  `reviews/DESIGN-REACHABILITY-DIAGNOSIS-AND-THE-RELAY-DECISION.md`. What to know:
  - **The observation needed no plumbing — the type was already in the data.** A gathered candidate
    arrives as its full SDP line and the `typ` token *is* the type. What bindings genuinely lacked
    was a way to *hand them out*: `IceObserver` + `MainThreadWebRtcEstablisher::with_ice_observer`
    (upstream `703168a`), teeing inside `drain_local_candidates` because that drain is
    **destructive** and by the end of a negotiation the session holds nothing. **No wire change** —
    this arm is main-thread throughout, so the native-shadow dance the design warned about never
    applied. Report on **success too**, or a consumer keeps showing "this network needs a relay"
    over a working connection: the cry-wolf failure arriving late instead of early.
  - **Parse the `typ` token by NAME, never by position** — the prefix carries optional components
    (`tcptype`, `raddr`/`rport` on reflexive candidates) and a positional read drifts the moment one
    appears. `prflx` counts as reflexive (it appears during *checks*, not gathering, and it is what
    makes symmetric↔cone work).
  - **THE STORE HOLDS ADVICE, NEVER CONNECTION STATE.** `dial_markers`' shape (in-memory, local,
    dropped on reload) — and `store::set` **drops every non-advisory verdict, `Connected` included**.
    A `Connected` row would make this map a second opinion about reachability, which is
    `connection_health` again. Liveness has one home (`system/peer/status`); this holds only the
    sentence we may *add* to it, and "nothing to add" is an absent row.
  - **IT NEEDED ITS OWN DIRTY SIGNAL, AND THE GATE IS WHAT FOUND THAT.** A verdict is written from
    inside the establisher's async negotiation — off the frame loop, no tree write, so no
    `WindowWatch` fires. First `make e2e-webrtc-nat` run against the feature: **~30 completed failed
    negotiations per side and the note rendered on exactly ONE of the two browsers**, because only
    that one happened to be repainted by something unrelated. Fixed with a generation counter
    compared once per frame in `dom/mod.rs` — same shape as `i18n::locale_generation` — bumped only
    on a *real* change, since a failing peer re-negotiates every few seconds and re-recording the
    same verdict must not force a full rebuild at that cadence. **Third appearance of the standing
    rule** (*every render input must carry its own dirty signal*): if you add another in-memory
    registry a surface reads, it owns a signal on day one.
  - **`make e2e-webrtc-nat` is the POSITIVE half** — the only gate proving the feature can produce a
    note at all, because everywhere else the requirement is silence, which a classifier wired to
    nothing also satisfies. It reads the **rendered text** (AP25 — an exit code cannot tell
    "classified correctly" from "classified correctly and rendered nowhere") and asserts the *right*
    note: no-reflector, **not** "needs a relay", since recommending a relay to someone who never
    configured a reflector sends them to buy the wrong thing. The four **"raises no false unreachable
    note"** assertions now cover the classifier too (`FALSE_NOTES` in `spike_meet_then_chat.py`);
    reword a string and you must update that tuple, or the check silently stops matching.
  - **Ordering inside `classify` is load-bearing**: an agent that gathered **nothing** is checked
    first and classifies `Unknown`. Written below the `reflectors_configured` arm at first, it called
    a never-started ICE agent `ReflectorUnreachable` — a test caught it before the gate could.
  - **`reflectors_configured` MEANS REFLECTORS, and the relay field made that a real distinction one
    commit later** [buildout 27]. The install site read `!ice_servers.is_empty()` — exactly right
    until `with_relay` made that list **mixed**, at which point a relay-only session reported a
    configured reflector and a host-only gather told the user *"the reflector did not answer"* about
    a reflector they never configured. It is `WebRtcProvisioning::has_reflector()` now, off the ONE
    discriminator `IceServer::is_relay` (credentials present) that `parse_relay` enforces and
    `pack_mirror` partitions on. **Neither feature's tests could see it** — the classifier's predate
    the relay, and the relay gate asserts `ice_servers` *counts*, never a verdict — so the bug lived
    in the seam and the audit found it, not the suite. Generalize it: **when a new field widens a
    collection, re-read every `is_empty()`/`len()` asked of that collection**, because those are
    claims about what is in it and they were written against the old contents.
  - **THE DIAGNOSIS IS MAIN-THREAD-ARM ONLY** [buildout 28]. `IceObserver` lives on
    `MainThreadWebRtcEstablisher` (`wasm-worker-proxy`); the Worker arm's establisher comes from
    `wasm-worker-host::build_webrtc_establisher`, which installs none — so under `?worker=1` every
    reachability failure is still the one sentence. The default browser arm is main-thread, so the
    shipped default has it; **the design note "no wire change — this arm is main-thread throughout"
    is true of the arm it was written about and false of the other one.** Closing it *is* the wire /
    native-shadow dance that note says never applied. Both diagnosis gates are main-thread, so no
    gate can see this either.
  - **A FAILED NEGOTIATION IS NOT A NETWORK FACT UNTIL SOMEBODY ANSWERS — `NoCounterpart`, and it
    is what the operator hit.** Every other arm is a claim about *the path between two peers*, and
    that path is not exercised until both are talking; the classifier could not see the difference,
    so a chat opened against a peer that was **simply not running** was told *"no reflector is set
    up, so this app can only reach devices on your local network"* — on one machine, where a
    reflector fixes nothing and none was the problem. Reproduced in a real browser 2026-08-22, and
    then fixed: `IceObserver::negotiation_finished` takes a `NegotiationReport` carrying
    `sdp_exchange_complete` (upstream `bindings/wasm-worker-proxy`, ours to fix), sourced from
    `WebRtcError::Timeout::answered`. Four things:
    - **`None` is *not measurable*, never `false`.** A carrier error or a policy refusal ends the
      negotiation before the loop can know, and reading that silence as *"they did not answer"*
      blames a peer for our own refusal. `classify` tests `== Some(false)` so `None` falls through
      to exactly the previous behaviour — which is also what keeps `e2e-webrtc-nat`'s `NoReflector`
      expectation intact.
    - **Ordering is load-bearing, again**: the empty-gather arm (about *us*) stays above it, and it
      stays above every network arm. Same rule as *"a never-started agent is not
      `ReflectorUnreachable`"* — the cheaper, truer claim wins, or a richer-sounding one gets made
      about a network nothing touched.
    - **Do NOT reach for `WebRtcError::Timeout::counterpart_msgs`** even though its doc says *"zero
      here is the genuine nobody-came case"*. §6.4 skip-own filters against **this negotiation's**
      own posts, so a peer retrying at the same pair key counts its **previous** negotiation's
      deposits — alive for their TTL — as the counterpart's. Measured: **8 "counterpart" messages
      against a peer that was not running at all**. `answered` is the honest discriminator.
    - **"HAVE THEY ANSWERED" IS STICKY PER PEER, AND THE FIRST FIX WAS WRONG WITHOUT IT.** A
      present-but-unreachable peer produces a **mixture**: some negotiations complete the exchange
      and fail at ICE, others never correlate (full bucket, retry storm, window closed first). The
      per-negotiation reading made the verdict flap — `make e2e-webrtc-nat` went red with **A saying
      "no reflector" and B saying "that device didn't answer" about each other, in the same
      topology, decided by which negotiation ran last** (B's first: `complete, fed=3`; B's last:
      `INCOMPLETE, fed=0`). So `store::ANSWERED_EVER` remembers that a counterpart has been heard
      from and `record_negotiation` folds it in; `classify` stays pure over one observation. Known
      limit, recorded: a peer that answered and then left keeps the network verdict for the session
      — expiring it would put this map back in the liveness business. **`fed == 0` is NOT a
      substitute discriminator** — B's last negotiation had it too.
    - **The gate is the browser, not a fixture.** The classifier's native tests were all green
      through the whole bug, because they assert the table and the table was internally consistent.
      What found it was loading the app over HTTP from the desktop's own app server and opening a
      chat with a real, offline peer-id — ~40 lines of WebDriver. And what found the *fix's* bug was
      `e2e-webrtc-nat`, the one gate that requires a note to appear: **a new arm in a classifier owes
      a run of the gate that asserts the OLD arm still fires**, because the new one can only take
      cases away from it.
  - **Still open:** the `NoDirectPath` gate (`e2e-webrtc-nat` **with** a reflector configured — the
    rig exists, the run does not); the surface is **Chat only** (Peer Connections and File
    Transfer's picker still say only *whether*); and there is **no `RelayUnreachable` arm**, so a
    configured relay that fails to allocate still gathers host+srflx and classifies `NoDirectPath` —
    *"this network needs a relay"* to someone who already rented one. Don't put the diagnosis on
    more surfaces before that arm exists, or the wrong sentence ships three times.
- **THE RELAY FIELDS EXIST NOW — `Connector.relay` / `.relay_username` / `.relay_credential`, and
  the entry below saying "the only missing piece is two text boxes" is CLOSED.** Three boxes on the
  connector row, `session_config::parse_relay`, and a second `IceServer` on the way to the agent.
  Five things to know:
  - **A relay is a SEPARATE field from `ice`, not a scheme you may type into it.** A reflector is a
    commodity: credential-free by spec (§9.3 forbids reflector authentication) and
    **node-advertisable**, which is what `ice_advertised` is and why its dedup is defined over
    published bytes exactly. A relay is rented, credentialed, and `EXTENSION-REGISTRY` §3b has **no
    credential channel**, so it can never be advertised. Both fields refuse the other's scheme and
    point at the right one.
  - **It rides as a SECOND `IceServer`, never merged.** An `RTCIceServer` carries one credential pair
    for all its urls, so folding a relay in beside reflectors either attaches the username to
    reflectors (which §9.3 forbids them to have) or drops it from the relay (which then gathers
    nothing). `WebRtcProvisioning::with_relay` is additive rather than a fourth argument to
    `resolve_webrtc_provisioning`, because only the registry path has a relay to supply.
  - **The refusal is the feature.** A relay URL with no credentials is accepted by
    `RTCPeerConnection`, shows as configured everywhere, and gathers **no relay candidates at all** —
    worse than an empty field, which is at least legible. Refused at `add_connector`, naming *which*
    half is missing (three separate fields exist so that message is possible).
  - **`pack_mirror` IS A SECOND, LOSSY EXPRESSION OF PROVISIONING — and it is what boot actually
    reads.** This cost the session's sharpest bug. `read_selection_mirror` reads **localStorage**,
    not the tree (the pre-peer boot path cannot do an async tree read), and the original
    `pack_mirror` flattened every entry's urls into one list and dropped `username`/`credential`. A
    relay came back as a bare `turn:` url, which `parse_ice_urls` then **refuses**, so the whole ICE
    list was discarded and the session fell back to host-only — against a connector row holding a
    perfectly good relay. **Every native test passed**; the tree round-trip is not the path boot
    takes. Caught only by `E2E_RELAY=… make e2e-webrtc-meet` reporting `ice_servers=0`. The general
    rule: **any new provisioning field must be added to the mirror as well, and the mirror partitions
    on `username.is_some()`** — the same discriminator `parse_relay` enforces, so the two cannot
    disagree.
  - **Gate: `E2E_RELAY` / `E2E_RELAY_USER` / `E2E_RELAY_CRED` on `e2e-webrtc-meet`**, typed into the
    Shell's `connector add` like `E2E_ICE`, asserted on **`ice_servers`** (entries) while reflectors
    are asserted on **`ice_urls`** (urls) — each needs its own counter, and reusing one would be
    blind to exactly what the other was added for. All three postures verified: none 0/0, relay-only
    **1 entry**, reflector+relay **2 entries / 2 urls**. It proves the *configuration* path end to
    end without a real TURN server, exactly as the reflector assertion does; **it does not prove
    relayed media flows** — that needs a relay to point at. **A counter is not a verdict, and that
    is what let the classifier bug through** — this gate was green in all three postures while a
    relay-only session was being told its reflector was down (see `reflectors_configured` above).
  - **A relay entry is discriminated by CREDENTIALS, in exactly one place** — `IceServer::is_relay`.
    `parse_relay` enforces it on the way in, `pack_mirror` partitions the mirror on it,
    `has_reflector`/`has_relay` read it back out. Three sites that must agree; the mirror already ate
    a relay once by disagreeing with the parser about what one looks like, so they call the same
    function rather than each spelling `username.is_some()`.
  - **The credential pair sits in localStorage in cleartext** (`pack_mirror` fields 4–6) — disclosed
    in the field's help text, not hidden, and not a new exposure *class* (the tree copy is equally
    readable to same-origin script, and the browser deployment ships no CSP). Recorded as a decision
    [buildout 29]: don't add a second secret to that mirror without meeting it.
- **TURN: we never host one, and the pipe to point at someone else's is ALREADY BUILT.**
  `WireIceServer` carries `username`/`credential` with an upstream `#[wasm_bindgen_test]` pinning the
  round-trip, and our `IceServer` shadow has both fields — **the only missing piece is two text boxes
  on the connector row**, the same bug as 2026-08-14's *"ICE was never blocked, it had no input box"*,
  one field along. Arch's ruling does **not** block it: §3b's absent credential channel is about a
  node **advertising** a relay it does not run; a user typing their own has no distribution problem.
  **What stays refused is bare `turn:` with no credentials** — it builds an `RTCIceServer` that looks
  configured and gathers no relay candidates. Two things usually stated wrong: a TURN server relays
  **ciphertext** (it sees who/when/how-much, never contents — so a rented commercial relay is a
  legitimate answer), and the browser's lever list is exactly host candidates, srflx, IPv6, and a
  relay — no listener, no port-mapping API.
- **"UPnP/NAT-PMP AND PORT FORWARDING ARE NOT LEVERS FOR US" IS WITHDRAWN — it was a claim about a
  browser tab dressed as a claim about the product.** We ship two peer forms and the whole NAT
  analysis was written about one. The **native** peer already binds `0.0.0.0:4041`
  (`WebSocketListener::bind`, `src-tauri/src/lib.rs`), already carries a raw-UDP §7 punch
  establisher (`core/peer/src/punch_establisher.rs`), and can speak IGD/NAT-PMP/PCP to a home
  gateway like every desktop P2P app since BitTorrent. Measured 2026-08-19:
  `upnp|nat-pmp|pcp|igd|port.?map` has **zero** occurrences in this tree — **unbuilt**, which is a
  different claim from unavailable, and the entry made the second one. Two things it hid: **(1)
  browser ↔ *reachable* native needs no WebRTC, no signaling node and no STUN at all** — the browser
  opens a plain WebSocket at the listener, which `e2e_worker` Phase 14/14b proves daily by moving
  files over it; port mapping is what turns "reachable" from *same LAN* into *anywhere*, making it
  the highest-leverage unbuilt item we have. **(2) Same-LAN browser↔browser still needs a
  rendezvous** — `e2e-webrtc-lan` removed STUN and TURN, not the node. Generally: **when an entry
  says "we cannot X", check which of our peer forms it is about.** Matrix by *pair* (not by NAT
  type) in `ANALYSIS-NAT-REACHABILITY…` §2a.
- **THE NATIVE PEER ASKS THE ROUTER FOR A DOOR NOW — `src-tauri/src/port_mapping.rs`, PCP + NAT-PMP,
  off by default.** Buildout 23. Zero new crates: NAT-PMP is a 12-byte request and a 16-byte reply,
  PCP 60 and 60, both to the default gateway on 5351 — smaller than the code that would configure a
  dependency, and fully testable against a fixture socket. Six things to know:
  - **COVERAGE IS "ROUTERS THAT SPEAK PCP OR NAT-PMP", WHICH IS NOT MOST OF THEM.** UPnP IGD is the
    widest-supported of the three and is **not** built (SSDP + SOAP + XML — the one place a
    dependency here is genuinely right). Don't state this feature as "port mapping works"; a
    IGD-only router reports *no mapping offered*, honest and useless. It is a second backend behind
    the same `Lease` — the lease, the CGNAT check, the withdrawal rule and the reporting are all
    protocol-independent and already built (23b).
  - **A MAPPING IS A LEASE, AND A FAILED RENEWAL WITHDRAWS THE ADDRESS.** Renew at half the
    **granted** lifetime, never half of what we asked for — routers quietly grant less, and using
    our own number lets mappings lapse. This is the listener-side twin of the transport-profile rule
    (*a wrong address is worse than none, because it is one the ladder spends a dial timeout on*);
    the temptation to keep the last good address across a failed renewal is exactly how it comes
    back. `only_an_open_door_has_an_address` pins it — `Probing` and `Closed` yield `None`.
  - **`is_publishable` RUNS ON THE SUCCESS, and CGNAT is why.** Behind carrier-grade NAT the gateway
    maps the port perfectly and hands back `100.64/10`: every protocol field says mapped, and a
    second NAT above drops everything. It gets its own sentence because *"your ISP has you behind
    CGNAT"* is a different conversation from *"your router said no"*, and without the address in the
    message the two are indistinguishable. Mutation-checked.
  - **PCP FIRST, and only `UNSUPP_VERSION` falls through to NAT-PMP.** Any other refusal is an
    **answer**; retrying it in an older dialect turns one clear "port mapping is off" into two
    confusing ones. PCP also carries the external address *in* the map response (NAT-PMP needs a
    second round trip) and has a **nonce**, which is the only field an off-path attacker cannot
    forge — NAT-PMP has no equivalent, so a forged external address is a real hazard there. The
    nonce check is the security property in that module; mutation-checked.
  - **Map the BOUND port, never the requested one.** The 4041 bind falls back to a dynamic port when
    taken, and mapping what we wanted rather than what we got forwards strangers to somebody else's
    listener. Same reason `stop()` **releases the door before dropping the listener** — a mapping
    that outlives its listener forwards strangers to a port nothing answers on.
  - **LINUX ONLY (`/proc/net/route`), and it says so rather than saying "no gateway"** — the failure
    is `Unsupported(os)`, because "no reader for this platform yet" and "your router refused" send a
    person to different boxes (23a). Read the table **little-endian** (`0102A8C0` is `192.168.2.1`,
    not `1.2.168.192`) and by **lowest metric** — a box with Wi-Fi and a VPN has several defaults.
    **The toggle shipped** (System Overview, beside the rendezvous row) and the row exists to keep
    **four `None`s apart** — not asked / still asking / refused / CGNAT — from `port_mapping` +
    `port_mapping_note`, all three derived from one `MapState` so a surface cannot show an address
    beside *"your router refused"*. Collapsing them would rebuild the one-sentence failure on a new
    surface. **Still open:** **no gate proves a mapping admits a stranger** (23d) — the rig's routers
    are iptables MASQUERADE and speak neither protocol, so that needs a responder fixture installing
    a real DNAT — and the failure `{why}` is English inside a localized frame (23f).
  - **A NEW MODULE'S FIRST COMMIT NEEDS ITS `mod` DECLARATION CHECKED, and nothing in the build will
    tell you.** `4d6e048` shipped `port_mapping.rs` — 1234 lines — with the `pub mod port_mapping;`
    missing, because a concurrent session in this same worktree ran `git checkout src-tauri/src/lib.rs`
    (which discards a whole file, not a line) between the last green run and the `git add`. Cargo
    does not compile a file no module declares, so `cargo check` was **clean**, the tree was
    **green**, and the commit message quoted a test count the compiler had never produced. Every
    other kind of missing code fails the build; this one is invisible, **and the bigger the module
    the more convincing the commit looks**. Two habits: `git diff --stat HEAD` before committing a
    new module and confirm the `mod` line is in it, and treat `git checkout <path>` as destructive to
    anyone else working the same tree.
- **TORI IS A SIGNALING NODE NOW — `src-tauri/src/signaling_node.rs`, off by default, one toggle
  in System Overview.** (The entry that said it *cannot* be one is closed; the distance really was
  a handler mount.) A desktop install can be the rendezvous for browsers that reach its listener,
  which is what turns "two people on one Wi-Fi need infrastructure" into "one of them has the
  desktop app open" — **federating** the rendezvous rather than hosting it. Five things to know
  before touching it:
  - **`mount()` returns the handler AND its seed policy together, and there is deliberately no way
    to take one** [AP22]. A node needs the handler mounted *and* an admission grant; they are
    separate builder methods, and splitting the decision is the shape that shipped a build
    resolving a node and installing no establisher. **The trap is invisible in a default build**:
    with `debug_open_grants` on (enforcement is opt-in), a handler mounted with *no* seed policy
    works for everyone and 403s the day anyone sets `ENTITY_BROWSER_ENFORCE`. This is the same bug
    `signaling_seed_grants`' own doc records the extension shipping, for the same reason — *"the
    in-process live tests seed a wildcard, which authorizes everything and so proves nothing about
    admission"*. **So `tests/signaling_node.rs` runs ENFORCED**, and is mutation-checked: drop the
    seed policy and the round-trip fails `403 capability_denied`.
  - **The listener now binds BEFORE the peer is built, and that order is load-bearing.**
    `advertise` publishes the node's endpoint verbatim for peers to dial, and the only dialable
    form is `connectable_addr(bound_addr)` — which does not exist until the bind picks a port.
    Build first and the node advertises `0.0.0.0`, which every peer stores faithfully and none can
    reach (`the_node_advertises_the_address_it_actually_listens_on` catches exactly that).
  - **The grant is narrow and must stay narrow**: `signaling_seed_grants` is the three signaling
    ops on `system/signaling` with an **empty resource scope**, seeded under `default` (a
    rendezvous only helps peers who have never met, so an allow-list is a contradiction). Turning
    the node on does **not** widen access to `local/files` — pinned by
    `the_seeded_grant_covers_signaling_only_and_never_the_share`.
  - **We publish no `reflection_endpoints`, because we run no STUN server.** Do not "helpfully"
    point that field at a public reflector — see the misuse entry below.
  - **The toggle restarts the backend**, because a handler cannot be added to a live peer. That is
    cheap (§1.3: a node holds nothing durable) but drops connections, so the UI says so, and the
    restart re-seeds the **remembered** manager peer-id (`BackendPeerRuntime.manager_peer_id`) —
    passing an empty one does not error, it logs a warning and skips the grant.
  - **These gates are in `make test-tauri`, NOT `make test`** — `src-tauri` is workspace-excluded,
    so a total quoted from `make test` (1175/15 binaries) does not include them. `make test-tauri`
    is **55/0 across 4 binaries** as of 2026-08-21 (+12 for `app_server` and the managed-peer
    filter). It was **43/0** on 2026-08-19 (and 28/4 until `port_mapping`
    landed its own tests — re-measure rather than quoting this).
- **THE RENDEZVOUS MAY BE BORROWABLE — the node's JOB is a generic ephemeral pub/sub, and `Carrier`
  is already a trait.** Do not restate "the signaling node is impossible to borrow" without this
  qualifier: it is true of *an entity signaling node* and false of what a node does.
  `SignalingCore` is `HashMap<RendezvousKey, Vec<Deposit>>` + `offer`/`collect` — **33** opaque key
  bytes (`RENDEZVOUS_KEY_LEN`; *not* the 32 every reader assumes from "it is a hash" — this seat
  wrote 32 in three docs and the first test to construct a key caught it), ≤8 KiB blobs, ≤32/key,
  60 s TTL, **no disk, no DB**, and the extension's own words are
  *"the node is mode-blind and derives nothing (§1)"*. Every piece of semantics (§3.1 key
  derivation, the envelope, the `hi`-waits-for-`lo` offerer rule, §11.5's single-flight bound) lives
  in the **peers and the payload**. So a Nostr relay / MQTT broker / BEP-44 DHT could carry it, and
  the change is **a second implementor of `punch::Carrier`** (`PeerCarrier`, `core/peer/src/
  carrier.rs`, is the first) — **nothing normative moves**. What you lose, name it before building:
  **admission control** (the node cap-checks `offer`/`collect` and enforces `bucket_full`; a public
  relay enforces its own limits), **limits discovery** (`advertise` publishes TTL/max-blob/lobby — a
  generic relay publishes nothing, and hardcoding walks into *"a limit the client does not know is a
  cross-impl reject boundary"*, where an oversize deposit presents as a rendezvous miss), and
  **operator choice**. Contents stay opaque either way — DTLS is peer-to-peer, no carrier is in that
  path. Ecosystem-wide there are exactly three answers and no fourth: **host it** (Tailscale/DERP,
  IPFS bootstrap), **federate it** (Matrix, Nostr, WebTorrent trackers, Syncthing), **distribute
  it** (BitTorrent mainline DHT, Jami/OpenDHT). We are already federated by construction.
- **STUN CANNOT SUBSTITUTE FOR A SIGNALING NODE — they are complements, and the question keeps
  arriving as if they were alternatives.** A STUN server answers "what is my mapped address" in one
  round trip and stores nothing; signaling needs a bulletin board (deposit at a key, collect 200 ms
  later). There is no STUN operation with storage. Off-LAN you need both; on-LAN you need only the
  node (`e2e-webrtc-lan`).
- **A node advertising SOMEONE ELSE'S public STUN is a MISUSE — the earlier "§4.5.1 says nothing
  about running what you advertise" is withdrawn.** Mechanically it works (the core publishes
  `reflection_endpoints` byte-for-byte with no ownership check, and it flows through
  `e2e-webrtc-advertised`), but `extensions/signaling/src/reflection.rs` reads §4.5.1 as *"this
  node's **own** listener(s), never a directory of anyone else's"* — vouching for infrastructure you
  do not run is `EXTENSION-REGISTRY` §3b's job, signed by the deployment identity for that reason —
  and the node CLI says *"omit it unless this deployment actually serves reflection"*. The surviving
  half is real: **running a node does not oblige you to run a reflector**; the suggested-STUN list
  belongs on our connector row or in a §3b signed set. **And no §9.3 STUN server exists anywhere in
  the ecosystem** — the flag advertises a listener nothing implements, and `e2e-webrtc-traverse`
  stands up a 30-line `stun_responder.py` fixture. A node operator serving reflection runs coturn
  beside it.
- **Public STUN is fine and must not be a DEFAULT — but the objection is consent, not security.** A
  hostile reflector can learn your IP and lie about your mapping (breaking connectivity, loudly); it
  cannot read anything, because the channel is DTLS between peers and STUN is not in that path. Keep
  it explicit because a default enrols a third party the user never chose (our posture everywhere
  else is fail-closed) and because public servers rate-limit and disappear. **And the infra question
  is usually conflated: STUN was never the piece that had to be ours.** A signaling node is
  protocol-specific and **cannot be borrowed from anyone**; a reflector is a commodity — and a node
  operator may advertise a *public* one under §4.5.1 (nothing says the node runs what it advertises,
  and unlike TURN there is nothing to vouch for), so **running a node does not oblige you to run a
  reflector.**
- **ICE servers are configurable now — the "always empty, nowhere to put them"
  note was true until 2026-08-14 and is WITHDRAWN.** The whole pipe was already
  built (`WebRtcProvisioning.ice_servers` → `app.rs` both boot sites → wire v2
  `WebRtcOpen.ice_servers` → `WebRtcSession::new`); the only thing missing was a
  *source*, so nobody could type a STUN URL. There is one now, on the **connector
  row** (`Connector.ice`, free text, parsed by
  `session_config::parse_ice_urls`) — a reflector belongs to the same operator as
  the node you rendezvous through, so switching nodes switches both halves
  together. All three provisioning sources carry it: the row, `?webrtc_ice=`,
  and `ENTITY_WEBRTC_ICE`. Proven end-to-end through the *shipped* surface —
  `E2E_ICE=stun:… make e2e-webrtc-meet` types it into the Shell's `connector add`
  and asserts the establisher installs with `ice_servers=1` on both browsers
  (unset asserts **0**, so the shared-bridge gates keep proving they enrol no
  third party). `stun:` only: TURN is **refused with its reason**, because there
  is nowhere to put a username/credential yet and an entry that gathers no relay
  candidates while looking configured is worse than a refusal.
  - **NAT traversal is PROVEN — `make e2e-webrtc-traverse` is the gate.** Two
    peers behind two separate NAT routers, each with its own external address,
    self-hosted STUN on a transit network: they meet at a name and exchange
    messages over WebRTC on the **first** negotiation, 5 offer deposits/side
    (inside §11.5's bound of 8), 3/3 runs. `tools/e2e/webrtc-rung1/nat_topology.sh`
    builds it and probes its own controls (no direct path, two DISTINCT external
    addresses, host reachable through each NAT) — a rig that silently degraded
    would "prove" host candidates traverse NATs. `e2e-webrtc-nat` stays as the
    **negative** control (no reflectors ⇒ no media); the two gates together are
    what separate "ICE is configured" from "ICE works".
  - **The lesson that cost the most: it was the RIG's NAT, not the app.** The
    routers ACCEPTED unsolicited inbound UDP, which confirms a conntrack entry
    holding the exact reply tuple `(external:advertised_port → peer)` that the
    peer's own outbound punch needs; the outbound then loses its advertised port
    and is remapped, so both sides send from ports the other never heard of and
    the punch can never converge. A real NAT drops that packet and keeps no
    state. **`bucket_full` (32 live messages) and `addIceCandidate: Unknown
    ufrag` were downstream of it** — retry-storm symptoms, not app bugs; both
    vanished when the punch started working, and the green run logs zero
    negotiation failures. Two general rules: **prove the network path with a
    bare UDP punch before blaming the application** (30 lines, and it is the
    only thing that separates the two), and a NAT rig must **measure** its
    mapping/filtering behaviour rather than assert it — a two-observer mapping
    test that varies only the destination *address* will call a symmetric NAT
    endpoint-independent.
  - **Node-advertised reflectors (§4.5.1) are BUILT — the automatic half works, and
    the note that this was "still upstream" is WITHDRAWN.** All three impls carry
    `reflection_endpoints` (rust `0b4e0cd`, pinned byte-identical against the Go
    node); a connector now *learns* what its node serves. Three rules, each of
    which cost a red gate or a design pass: **(1) the learning has to ride a path
    the user already walks** — recording only on the `Check` button left the gate
    red, because adding a connector and meeting by name never asks the node, so
    `learn_node_reflectors` fires on **add** too (a "Check first" requirement is
    the automatic half wearing a manual hat). **(2) The typed and advertised
    halves are separate fields** (`Connector.ice` / `.ice_advertised`), merged at
    read by `merge_reflectors` — union, typed first, deduped on **published bytes
    exactly** (no case-folding, no default-port canonicalization; normalizing
    breaks the identical dedup `EXTENSION-REGISTRY` §3b relies on). `add_connector`
    **ignores** a caller's `ice_advertised` and preserves the stored one, so
    editing a label cannot silently drop you to host-only —
    `re_adding_a_connector_keeps_what_the_node_advertised`, mutation-checked.
    **(3) A malformed *advertised* entry is dropped alone**; only a *typed* list is
    refused whole, at `add_connector`, where a human can fix it.
    Gate: **`make e2e-webrtc-advertised`** — the node runs with
    `--reflection-endpoint` and the browsers type **nothing**. It asserts on
    **`ice_urls`, not `ice_servers`**: `parse_ice_urls` packs every reflector into
    ONE `IceServer`, so `ice_servers` is 0-or-1 by construction and would pass for
    any non-empty list. All four postures verified — none 0, advertised-only 1,
    typed+advertised same-uri **1 (dedup)**, different **2 (union)**. Still
    upstream, and unchanged: `Advertisement` carries only `endpoint` + `limits`,
    and §3b's `endpoint` is an object while §3b.3's byte-pinned MUST hashes a
    *string*. That fork must settle before anyone decodes it. It fills in
    `Connector.ice`; it is not a prerequisite for having ICE at all.
- **Calling a signaling node needs a ROUTE first, and the registry lives on the
  system peer.** A connector is reached by `entity://` URI like any other remote
  peer, so the ladder wants rung 1 (pooled connection) or rung 2 (a published
  transport profile) — a connector the user just added has **neither**, and rung 4
  is circular for the rendezvous node itself. The failure reads *"no transport
  profile for peer"*, which sounds like the node is down when we simply never
  called it: dial via `connectors::reach_node` (skips when the kernel liveness
  read-model says `connected`, because `connect_peer` opens a NEW connection each
  call) before any `advertise` / `offer` / `collect`. Split of concerns to keep:
  the **registry + selection are read/written on the SYSTEM peer** (deployment
  infrastructure — provisioning, the window and `ConnectorRegistry` all use it;
  the shell verb used the *bound* peer and so quietly managed a second registry),
  while the **call itself dispatches from the acting/bound peer**, whose pool,
  grants and id it rides — that id is what a counterpart comes away with from a
  `meet`. (`src/connectors.rs`, `src/rendezvous.rs`,
  `STATUS-2026-08-13-naming-modes-meet-at-a-name`.)
- **RENDEZVOUS IS NOT THE ONLY MEDIATED SUBSTRATE — `EXTENSION-RELAY` Mode S is landed, implemented
  and platform-neutral, and we consume NONE of it.** Measured 2026-08-20 against core-rust
  `302b7f4`: `extensions/relay/src/handler.rs` dispatches `forward` · `put` · `poll` · `advertise` and carries
  **no `cfg(target_arch)` gate**, so the client half builds for wasm today. Mode S is *put-then-poll*
  — a sender `put`s an envelope, the recipient `poll`s it — which needs **no rendezvous, no both-ids
  requirement and no simultaneity**, and RELAY §2 files static-CDN-hosted peers exactly there.
  `EXTENSION-ROUTE` is the next-hop table (`match`/`action`/`via`/`metric`) that makes Mode F
  SMTP-shaped multi-hop. **So "how does a browser peer receive anything when nobody is standing at a
  bucket" has a second, spec-landed answer nobody here has built** — `grep -rniE
  "system/relay|system/route|system/inbox" src/` returns **one comment** in the Knowledge Base view.
  Two things to carry: **(1)** don't describe our reachability story as "rendezvous or nothing" — it
  is "rendezvous, and we have not built the other one"; **(2)** Mode S does *not* close the
  stranger-holding-only-a-peer-id gap, it fails on the **same** missing piece — the querier must
  learn *which relay we poll* (`inbox-relay`) exactly as they must learn *which signaling pool we
  use*. **One missing field shape across two extensions: a peer-id-keyed locator for an
  intermediary.** Routed as `ROUTING-2026-08-20-e` §3; arch folded it and it is **three** rungs, not
  two — `EXTENSION-RELAY` §3.5 had already written its own instance down (*"reachable by a stranger
  only via a cached copy — honest parity with a mail server that has no DNS entry"*). **The parity is
  what breaks: a mail server with no DNS entry has no identity you can hold either, while here you
  hold a `peer_id` that self-certifies every answer it could be given. The lookup is missing, not the
  identity.** Frame now documented at `GUIDE-NETWORKING-MODEL` §4a. Before proposing any new exchange
  mechanism, read `EXTENSION-RELAY` §1–§2 and `reviews/DESIGN-STATIC-TRANSPORT-AS-RELAY.md` first —
  the four-mode set is deliberately *design-space, not a canonical pick*, and most "we should also
  support X" ideas are already one of its configurations.
  - **RUNG 3 DOES NOT WAIT ON THE LOCATOR — Mode S reaches us TODAY from any party that resolved our
    NAME**; the peer-id-keyed locator (arch R-25) is only the `peer_id`-*only* case. So "build the
    relay client" and "wait for the transport-set to be ruled" are independent, and treating rung 3
    as blocked on R-10 would be wrong. Not on the v1 release line (that is registry lookup + site
    path), available whenever we want it.
  - **A MODULE LIST IS NOT A COMPLETENESS CLAIM, and this entry made that error in its first
    version.** It said *"relay+route+inbox in rust, relay+inbox in go"*, which reads as core-go owing
    a ROUTE catch-up. **It does not: `EXTENSION-ROUTE` defines NO operations** — §2, *"the table is
    the entities; a read is a standard `tree:get`… this entity shape + the match (§3) + the cap (§5)
    is the entire v1 conformance surface."* Verified in go's own tree, not taken from the packet:
    `core/types/route.go` (the entity), `cmd/internal/validate/route.go` (declared ROUTE-EXACT-1 /
    ROUTE-ABSENT-TABLE-1 checks), and the consumer doing the §3 table read in `ext/relay/relay.go`.
    rust's `extensions/route/` and py's `route.py` are a **factoring choice**. **When we build: we
    need the entity type, the §3 match and the §5 cap — not a route module.** Same shape as this
    file's two standing warnings that a **`cfg` line** and a **constant name** are not the code path:
    *a directory listing is not the conformance surface* — read the spec's own conformance sentence,
    then the tree.

## Publishing, signed roots & names

- **A RE-KEY BRICKS EVERY RETURNING VISITOR, AND THE SELF-HEAL YOU ALREADY WROTE MAY BE WHAT
  STOPS IT.** If a domain publishes under a new identity, a returning browser keeps asking the
  retired one forever: newest WASM, 404s, an app that reports healthy. Live incident
  `ecdeos.org` 2026-08-24; fixed 2026-08-27 (**AP26**, resilience design §1.1a/§1.1b). Four
  things worth carrying:
  - **`app.rs` re-reads `/entity-deployment.json` whenever the home is a REMOTE peer** and
    adopts on **identity** divergence. Do not re-introduce a precondition in front of it. The
    old (1.2.5) reconcile had one — *only if the home peer's origin is unregistered* — which
    repaired the origin for the peer you already have and could never repair *the peer you
    have being wrong*. Its guard was false on the incident, so the general path never ran.
    **It is retired, not moved**; do not add a sibling beside the new one.
  - **Adopt routing facts only** — `home_site`, `origins`, the registry pin. Posture
    (`site_mode`, `surface`, `window_type`) is a user preference and must not move. An
    absent/empty `home_site.peer` is **not** a divergence, or a truncated doc could re-home a
    healthy browser.
  - **The session config is only half the repair, and the other half is NOT per-surface.**
    Nav state persists its own `peer` (`ContentSiteState`), so fixing the config alone parks
    the user on the retired publisher *while the config reports healthy*. The first attempt
    re-pointed the overlay only and left a Site Browser **window** broken — do not do that
    again. **A peer being replaced is one fact about a peer, not N facts about surfaces:**
    it is recorded once in `src/peer_supersession.rs` and resolved in
    `ContentSiteState::from_entity`, the single decode point every surface shares.
  - **Do not "just sweep" the stored state.** It cannot be made correct on both arms — the
    recursive enumeration is the *sync* `tree_listing` (Worker-arm mirror not reliably
    seeded at boot) and `tree_listing_async` returns **immediate children only**, with
    directory entries dropped at the worker boundary. A sweep silently misses entries on one
    arm. The supersession record has no such failure mode: its only enumeration is of a flat
    one-level registry.
  - **The record must be DURABLE.** The adoption branch runs exactly once — the next boot's
    config already agrees with the domain, so there is no divergence left to detect. An
    in-memory-only record repairs whatever happened to be open and nothing else, ever.
  - **Two paths can share one console.** `deployment` being `None` and `Some`-without-the-key
    both collapse through `unwrap_or(false)` onto the same `app.rs:2145` warn, which is how
    two documents concluded the reconcile had fired when it had not. The discriminator was a
    single *absent* log line. When you fix a boot path, check what its failure looks like from
    outside before trusting a log to identify which branch ran.
  - Gates: `rekeyed_domain_heals_on_next_boot` **and `…_window_surface`** — the same
    scenario over both deployment shapes, because one surface is how this was got wrong.
    They need no domain. Three harness traps they cost, all worth knowing before you add a
    fixture: a libtest **substring** filter also matches a longer test name; `--exact`
    compares the **fully-qualified** name and libtest **exits 0 when it matches nothing**
    (assert `1 passed`); and a fixture that publishes into the shared `dist/` breaks Phase 27
    with *"publisher bound no signature"* — stage an isolated copy on its own port instead.

- **`transports` IS `[system/hash]`, NOT INLINE PROFILES — REGISTRY v1.21 D8/D8a/D8b, and we are
  the emitter the whole cohort waits on.** A binding now *references* `system/peer/transport/*`
  entities instead of carrying endpoint descriptors inline (arch `c2eb423`; `ROUTING-2026-08-21-f`
  put browser-rust at step 1 of a four-seat chain — *"nothing downstream can be measured until you
  re-emit"*). Six things, and four of them cost something:
  - **What we GAIN is the point, not the bookkeeping**: the profile arrives carrying its **entity
    type**, so a consumer runs `EXTENSION-NETWORK` §6.5.1a D5's fail-closed `transport_type` check
    against a typed entity instead of trusting an unsigned inner field.
  - **D8a — a publishing registry MUST serve what its bindings reference**, and *"a missing referent
    and a withheld one are byte-identical at the consumer"*. So `SignedSession::content` (the new
    by-hash counterpart to the by-key walk) classifies a missing referent through
    `declared_fetch_error` → **`IncompleteWalk`**, never `Transport` and never a degraded
    `Ok(None)`. Sixth appearance of the standing seam; the rule is unchanged and the surface is new.
  - **`ecf_for_hash`/`to_ecf`, NEVER `ciborium::into_writer`, for anything that will be fetched by
    hash.** `verify_and_decode` canonically **re-encodes** the decoded `data` before hashing, so a
    body encoded any other way fails as `HashMismatch` — *the origin served bytes that do not hash
    to the address it committed to*, said about our own correct emission. The old encoding was
    invisible for a year because the standalone `transport-profile` artifact is **not**
    hash-verified. **When an artifact gains a by-hash consumer, re-check how its bytes were made.**
  - **`write_entity` puts a blob on disk; only `root.record` puts it in the SIGNED ROOT'S CLOSURE**
    — and a gate that checks the file exists cannot tell them apart. The first version of
    `a_registry_serves_the_transport_profiles_its_bindings_reference` asserted disk presence and
    **survived deleting the `record` call**; it resolves the profile *through the session* now, and
    that mutation is red (`Absent`). A served-but-uncommitted entity is fetchable, unverifiable, and
    indistinguishable from one the origin invented.
  - **The `.list` artifact is a `system/tree/listing` WIRE ENTITY** (§6.5.3.1 — *"No JSON form"*),
    not newline text. Its `entries` map is canonical CBOR, so key order is **length-first, not
    alphabetical** — `list_names` sorts for the reader rather than the emitter emitting a
    non-canonical map. Decode it with `ciborium`, never `entity_wire::decode_entity` (not linked on
    wasm32; `data` arrives as a map on some arms and a `bstr` on others).
  - **`make federation-vectors` commits the corpus** to `tests/fixtures/registry-federation/`, with
    `--issued-at=` pinning the clock (`issued_at` rides every binding body → every binding hash →
    the trie shape). Everything is byte-reproducible **except** `published-root` and its signature —
    measured, 40 of 285 files' worth of lines — because `PublishedRootData.published_at` is a
    wall clock inside upstream `Peer::publish_root`. Treat a diff **anywhere else** as a real
    emitter change. Read that README before regenerating.
- **THE REGISTRY PIN HAD NO GUI AND TWO SURFACES DISAGREED ABOUT IT — one pin now,
  `session_config::pinned_registry()`.** The Registry Browser read only the deployment seed while
  the Shell's `name pin` wrote a `Mutex` on **that one window**, so pinning in one surface left the
  other saying *"No registry pinned"* — with no field anywhere in the GUI to set one. That window's
  own comment recorded the split and declined to resolve it ("deliberately not answered by
  duplicating the slot here"): right not to duplicate, and the answer is to have **one**.
  `ShellModel::name_pin` is **deleted**, not left as a second store nobody reads. Four riders:
  it **persists** across a reload through a localStorage mirror (the `write_selection_mirror` /
  `boot_fast_paint` idiom, and for their reason — the shell resolves inside a `spawn_task` holding
  no config handle, and a Worker-arm boot-time tree read returns the default); restore happens in
  `app.rs` **beside** the deployment seed so the precedence lives in one place; **`name unpin` is a
  consequence of persisting, not a nicety** — a choice you cannot withdraw is worse than one that
  evaporates, and it reports what you fall back *to*, since unpinning does not mean "no registry"
  when the deployment seeds one; and an **empty origin is accepted** (same-origin, as in the
  `origins` map) while an empty or keyless peer-id is refused **with its reason on screen** —
  a Pin button that silently does nothing is the operator-surface failure this file keeps
  recording. **Any new surface naming a registry calls `pinned_registry()`; none keeps a slot.**
- **A static publish carries a SIGNED ROOT now, and two of its shapes will bite you.** `make
  site` / `entity-browser publish` emits the HAMT trie closure plus a signed
  `system/peer/published-root` (`src/content_site/signed_root.rs`, B14), so a consumer pins the
  publisher key and the *origin becomes untrusted*. Two things that are not obvious:
  **(1) trie interior nodes appear in NO location-index listing** — they are reachable only by
  hash from the signed root, so any projector that enumerates bindings emits a tree that cannot
  be walked, and it fails as `Ok(None)`, indistinguishable from "that page does not exist". That
  is what B14 was; don't reintroduce it by adding an emit path that skips `write_entity` (the
  choke point the `RootProjector` is fed from). **(2) The manifest is a `.bin` that is NOT a
  `system/hash` pointer** — it is the 3-key *wire* entity, because `verify_signed_root` reads its
  `content_hash`, which the bare hashable form does not carry. `--verify` excludes it from the
  pointer sweep by path and checks it separately; a sweep that forgets reports the trust chain's
  own anchor as BROKEN. **(3) `--verify` fails on an incomplete closure — and this entry claimed
  that a day before it was true.** "Every pointer resolves" and "the tree is walkable" are
  independent properties, and the guard for the second one **could not fire**: the closure walk
  discovered children by scanning 33-byte windows and enqueued a candidate only when
  `fetcher.content(&c).is_ok()` — *only when already present* — so `missing` was reachable for
  exactly one hash, the root. Measured on a 24-name registry: withholding one interior node took
  the closure 56 → **54 blobs, `0 missing`, exit 0**. The presence filter was load-bearing for
  *discovery* (the scan is structure-agnostic, so random bytes parse as hashes) and fatal to
  *completeness* — one loop doing two jobs with opposite requirements. **Fixed by separating them**
  (`AUDIT-NAMING-AND-PUBLISHING-ARC-2026-08-18` F8): discovery keeps the scan, completeness is
  **structural** — gate on `entity_type == TYPE_TREE_SNAPSHOT_NODE`, decode with
  `SnapshotNodeData::from_cbor`, and require every declared `Bucket` value and `Link` to exist.
  Same case now reports `1 missing`, names the withheld hash **and the parent that declared it**,
  exits 2. The old comment justifying the heuristic — *"a publisher must not need to know the HAMT
  encoding to check its own emit"* — is exactly what cost us the guard; the decoder is upstream and
  public. Gate: `registry_verify_passes_a_clean_tree_and_fails_a_withheld_interior_node`
  (mutation-checked both ways). **Fourth appearance of one seam** — B14's `Ok(None)`, upstream
  `collect_bindings_into`, `--verify` over an incomplete closure, and now the check written to
  close that third one: *"absent" and "withheld" keep arriving as the same value.*
- **DO NOT split our publish into two roots — `EXTENSION-TREE` §3.3a's extent MUST is REFUTED, and
  the refutation is `ROUTING-2026-08-18-m`.** arch's `-l` §4 assigned us exactly that action (our
  site root commits to `sites/**` + `apps/**` under one `/{peer_id}/` claim, which they called a
  false extent claim). We went to do it; the rule does not survive being read, and the text still
  stands at arch HEAD — so **a seat reading the spec cold will be told to restructure. Don't.**
  Four independent failures, each sufficient: **(1) it is unsatisfiable by construction** —
  `PublishRootEngine::publish` binds `/{pid}/system/peer/published-root` and
  `/{pid}/system/signature/{hex}` in the location index, and neither can be in the committed trie,
  because the manifest hash **H** is computed *over* `root_hash` and the signature is over **H**;
  an anchor cannot sit inside the tree it anchors. That makes `Peer::publish_root`'s own default
  prefix (`/{peer_id}/`) non-conformant for every publisher in the ecosystem. **(2) Its
  consumer-side justification is revoked by the next paragraph of the same section in the same
  commit (`8bfc9b6`)** — the extent MUST says a consumer "is entitled to read [a negative] as
  authoritative"; the negative-scoping MUST says a consumer "MUST NOT present a negative … as
  authoritative non-existence." The harm it names was already closed, better, four lines down.
  **(3) It forbids what that paragraph permits** (per-audience subsets are incomplete under their
  prefix by definition). **(4) "Disjoint areas" is undefined** — `sites/a/` and `sites/b/` are
  disjoint in exactly the sense `sites/` and `system/registry/` are, so the rule degenerates to one
  root per key, and §12 has no test for it. The remedy is also unexpressible: **one
  `published-root` path per peer** (`published_root_head_path` takes a peer-id and nothing else),
  so "N areas → N roots" means N identities → N pins, against REGISTRY's `name → peer-id` model.
  **The generalizable lesson, and it is the one that cost the session:** *a landed MUST is a claim,
  not a fact.* We had just finished telling arch to verify a spec claim in `specs/` rather than in
  the packet describing it — and then took a MUST from `specs/` on its word and started
  implementing. **Read the artifact the rule is about before you obey the rule**; here that was 40
  lines of `published_root.rs`, and it refutes the rule outright. Note also that our own emitter is
  *not* what they thought: the publish identity is dedicated (`persistence::publisher_keypair`,
  `{ENTITY_DATA_DIR}/publish/`) and `RootProjector` builds a fresh peer that skips foreign peers, so
  the harm they described — a consumer hitting one of our *app-state* keys — cannot occur; that
  identity has no app state.
- **THE MANIFEST'S LOCATION IS DISCOVERED, NEVER DERIVED — and a green publish/consume pair inside
  one arm proves only that we agree with ourselves.** `EXTENSION-NETWORK` §6.5.3 **v1.8**: *a
  consumer MUST NOT join `signed_pointer` onto an origin*; `signed_pointer` says **what** the origin
  asserts (the §3.3a tree path), `manifest_url_prefix` says **where to GET it**, and
  `{origin}/manifest` is as conformant as `{origin}/{peer}/system/peer/published-root`. Ours derived
  it by convention, so the first real cross-impl run — workbench-go's fixture, vendored at
  `tests/fixtures/crossimpl-go-site/`, walked by `src/content_site/crossimpl_go.rs` — **failed at
  hop 0**, hiding every surface the two arms *do* agree on (content sharding, the bare-hashable body,
  the two-hop signature keyed on the published-root entity — all three confirmed once the door
  opened). `publish_layout.rs` reads the profile now; the convention survives **only** as a named
  absence-of-source fallback (`name pin <pid> <origin>` — the user typed a URL, there is no endpoint
  document to read). Two refusals are load-bearing: a profile with no `manifest_url_prefix` is **not**
  completed from the convention (that *is* the violation), and an unknown `content_layout` is **not**
  assumed sharded — a wrong guess 404s every blob, which reads as a withholding origin. **Decode the
  profile with `ciborium`, never `entity_wire::decode_entity`** — it is not linked on wasm32, and the
  artifact's `data` arrives as an inline map on the Go arm and a `bstr` here; accept both.
  **BOTH HALVES ARE CLOSED NOW** — the consumer (`PinnedPublisher` carries a `PublishLayout`, not an
  origin string; `named_site::http_poll_layout` hands over what the binding advertised;
  `session_cache::session_for_layout` is the binding-resolved entry point) and the publisher
  (`signed_root::write_transport_profile` emits `{out}/transport-profile` after the root is signed,
  and `http_poll_profile` now carries `signed_pointer`). Gates, both mutation-checked:
  `the_shipped_consumer_resolves_a_go_published_page_over_their_advertised_layout` (restore the
  convention → 404 at hop 0) and `a_publish_advertises_its_own_endpoint_and_a_consumer_enters_through_it`
  (drop the emit → fails on the artifact, not on the walk). **`http_poll_origin` is gone and its F6
  verdict was wrong** — it refused any prefix not ending in `/{peer-id}` as *"a layout we cannot
  consume"*, which is every conformant publisher that is not our emitter. The *rule* survives as
  `PublishLayout::origin_for` (act only when the last segment is **exactly** the peer being pinned,
  or `"https://x.example"` truncates to `"https:/"`); what died is treating the other form as
  unconsumable rather than as origin-rooted. Note the guard F6 also carried — `!head.is_empty()` —
  **was itself a bug**: a same-origin publish advertises `tree_url_prefix: "/{peer_id}"`, whose head
  is empty and which is peer-rooted all the same; `last == peer_id` is the whole discriminator.
  **`system/peer/transport/http-poll` is not an upstream constant** (`core/peer` has
  `TYPE_PEER_TRANSPORT_{TCP,HTTP}` only — the same "spec landed, Rust type did not" shape as the
  missing `…/transport/websocket`); the string is pinned against workbench-go's emitted artifact,
  read out of it rather than assumed. **Three riders.** (1) `session_for_layout` inherits
  first-wins: a publisher first met at a *typed* origin keeps the convention layout even if a later
  binding advertises a real one — the floor outranks the layout, and the order of first contact
  decides. (2) **`tree_url_prefix` is ambiguous in the same section** —
  the normative sentence joins `{tree_url_prefix}/{peer_id}/{path}` (origin-rooted, their emission)
  while the worked example and §6.5.3.1 step 5 join `{tree_url_prefix}/{tree-path}` with the peer-id
  baked into the prefix (ours). Same files, opposite reading of who appends the peer; routed
  (`ROUTING-2026-08-19-c` §3), not patched. (3) **A source-read prediction has a shelf life:** their
  §3 predicted `EISDIR` from a directory at that path, and by our run core-go `2bd2380` had dropped
  the peer-id segment, so it was `ENOENT` on a file sitting right beside us — invalidated by a shared
  dependency bump, with neither side's code changing. The prediction was right about the *location*
  and stale about the *mechanism*; that is the argument for running it, not for reading harder.
- **Consuming a signed root is a PUMP, and it needs a session — `src/content_site/signed_fetch.rs`
  (B15).** `PublishedRootClient::resolve` is sync and `fetch()` is async, with no async twin
  upstream, so the walk is *driven*: resolve → collect misses → await → resolve again,
  terminating because each round either resolves or strictly grows the cache. Three traps, each
  of which fails silently: **(1) a cache miss surfaces two different ways, oppositely** —
  `Ok(None)` inside the walk (same as a key that is genuinely absent) and `Err(Fetch)` on the
  final leaf (same as a real transport failure), so the **miss log is the termination condition**,
  not an optimisation, and handling only `Ok(None)` fails on every page. **(2) `signature_for`
  must return `Err` on a miss, never `Ok(None)`** — `Ok(None)` reaches `verify_signed_root` as
  *"this origin serves no signature"*, which is terminal, so a pump built on it reports an
  unsigned origin the first time the signature merely had not been fetched yet. **(3) A
  verification failure is terminal** — only `PublishedRootError::Fetch` re-enters the pump, or a
  hostile origin gets unbounded attempts to be believed.
  **Hold a `SignedSession` across page loads; never build a client per fetch.** The `seq` floor
  lives for the lifetime of the client, so a per-call client lets an origin roll a site back
  *page by page* — `seq 1` for one page, `seq 0` for the next — while every signature and every
  hash still verifies, because both trees really were published by that key. Nothing else in the
  chain catches it. The session also carries the content cache: measured **5 fetches for the
  first page, 2 for the second**.
- **A registry is the same emitter at different keys — `entity-browser registry` (B16a).** Signed
  `name → peer-id` bindings plus a signed root over the registry's own tree
  (`src/content_site/registry_publish.rs`); §7.4's *"the registry is itself a coral reef"*. Three
  rules learned on it: **the registry has its OWN durable identity** (`{ENTITY_DATA_DIR}/registry/`,
  not `publish/`) — a consumer pins name-issuer and content-publisher independently, and one key
  doing both means trusting a name-issuer to also be the thing it names; **the by-name index is
  one entity under TWO trie keys**, so it is `record_hash`, never a second `record` (emitting the
  body twice makes the content address stop being the dedup); and **the identity entity must be
  projected** (`put_only`) or every signature verifies nothing, because `sig.signer` resolves
  through it. **`ttl` and `issued_at` are MILLISECONDS** — `resolve_one` checks
  `issued_at + ttl <= now_ms()`, so emitting seconds yields a permanently-expired binding whose
  resolve returns `None`: the same answer as a bad signature or a revocation, so it reads as a
  broken registry. Nothing in §6a says so; it was found by running the chain. **`ttl: null` is
  deliberately not expressible** (arch D3) — a null-TTL binding whose revocation a hostile origin
  withholds is permanently unrevokable, which is why the *"bounded by ttl + revocation"* argument
  is void. **The consumer half (a default resolver-chain entry) is NOT ours to ship yet** — it
  waits on arch pinning the default `name_format_dispatch` globs, or two app tiers ship two and
  private names leak to the default registry. **We stopped waiting and wrote the proposal** — see
  the `name_format_dispatch` entry below; the mechanism ships, the default does not.
- **"THE ABSENCE OF A NODE IS NEVER AN ANSWER" — arch's §4 invariant, and our CONSUMER already
  satisfies it (their disposition for us was wrong).** `ROUTING-2026-08-18-j`: a node that
  *resolved* without the key is `not_found`; a node that *did not resolve* is a failed walk, and
  the two must never be the same value. Arch dispositioned our consumer side as *"closes when the
  engines land it"*, assuming we inherit `collect_bindings_into`'s tolerant walk. **We do not** —
  resolution is `trie_get` behind `SignedSession`'s **pump**, so a miss inside the walk is recorded,
  the pump then *fetches* it, and a withheld blob becomes **`IncompleteWalk`** (it was `Transport`
  until the discriminator landed — see the next entry). `Absent` is reachable only when the walk
  completed with **nothing outstanding**. That is what makes the revocation probe fail closed: a
  withheld node lands in `NameError::Registry`, not the `Absent` arm meaning "not revoked".
  **The gate that pins it was written the wrong way round for a session:**
  `an_unwalkable_tree_terminates_rather_than_spinning` asserted `Transport | Absent` and its doc
  said the two were *"the same observable"* — the invariant stated backwards. Tightened to
  `Transport` alone it **passed unchanged**, so the tolerance never described our behaviour; it has
  since been tightened a second time, to `IncompleteWalk`. Mutation-checked at each step. Two
  riders: **enumeration is the
  half that is genuinely open** (a signed name listing would go through `collect_all_bindings`,
  which *is* the tolerant walk — so an unresolvable entry there must be a FAILURE, never the end of
  a branch); and the **published-then-withheld revocation is a FIXTURE GAP, not a proven property**
  — `emit_registry` cannot emit a revocation, so the one shape where a blob sits *only* on the
  revocation path is untested, named at `the_revocation_probe_adds_no_fetches_of_its_own`. Measured
  there: a full `resolve_name` is manifest + signature + **3 content blobs** + manifest, and the
  probe adds **zero** — so no blob starves only the probe, and hop 1 fails first.
- **A WITHHELD BLOB AND AN UNREACHABLE ORIGIN ARE DIFFERENT ANSWERS — `declared_fetch_error` is the
  one place that decides, and it is TERMINAL vs RETRYABLE.** arch ruled it (`EXTENSION-TREE` §3.3a
  via `ROUTING-2026-08-18-l` §5, our line): an **incomplete walk** is the origin failing to produce
  what its own signed root declares, and retrying grants a hostile origin unbounded attempts while
  turning a withholding into a hang; a **transport** failure fetching a blob is separately
  retryable. We could not express it — `PollError` had no HTTP-status variant, so a 404 became
  `Decode("HTTP 404 …")` → `SignedFetchError::Transport`, and *withheld* and *unreachable* arrived
  at the caller as one value. **The signal was never missing**, it was discarded one layer down: an
  origin that answers 404 has *chosen*; one that drops the connection has not. Now
  `PollError::NotFound(u16)` → `SignedFetchError::IncompleteWalk`, classified in exactly one
  function so a new call site cannot re-collapse it by reaching for `Transport` out of habit.
  **Only 404/410 map to terminal, deliberately** — a 403 or 5xx on a declared blob may be a
  misconfigured CDN that serves it next minute, and calling those terminal converts a transient
  origin fault into a permanent resolution failure. **The manifest fetch is deliberately excluded**:
  incompleteness is only definable against a root you already hold, so a 404 there means the origin
  serves no published root for this publisher at all — an unreachable publisher, not a short walk.
  Gate: `a_withheld_blob_is_terminal_while_an_unreachable_origin_stays_retryable` — the **same**
  declared blob failing two ways must classify two ways, over **every** blob the walk declares (3
  measured), mutation-checked in *both* directions. **Its first draft picked `declared[0]` and
  silently only ever exercised the signature path** — under hash-keyed routing "the first blob" is
  not a stable referent, the same lesson as picking a structural victim by effect rather than
  position. **This was the fourth appearance of one seam** (B14's `Ok(None)`; `--verify` over an
  incomplete closure; the structural fix for that which could not fire; now this), and it is the
  first time the fix was to give the two outcomes *different types* rather than to make
  incompleteness an error. **The fifth arrived (2026-08-19) and it is the same move: a CORRUPT
  body was reported as an unbound NAME.** A flipped byte in a HAMT **interior** node resolved to
  `NameError::NotBound` — *nobody has claimed that name*, said about an origin serving bytes
  matching no hash it committed to. The leaf was never at risk (upstream hashes it →
  `ContentHashMismatch`, terminal); the interior node is, because
  `VerifyingFetchStore::get` (`core/peer`) does `verify_content(..).ok()?` — a mismatch becomes
  `None`, the trie walk reads that as *no such branch*, `resolve` returns `Ok(None)`, and with the
  bad bytes already in our cache **no miss is outstanding to re-drive the pump**, so the walk ends
  `Absent`. Fixed in BOTH halves: `SignedSession::resolve` verifies a declared blob **before admitting
  it to the cache**, and **upstream** (`core/peer` `302b7f4`) the store now latches the mismatch and
  `resolve` checks it before believing a `None` — so every consumer of a signed root gets the honest
  answer, not just us. The upstream half needed an `AtomicBool`, not a `Cell`, because
  `ContentStore` is `Send + Sync`; and its gate asserts the **control** as hard as the attack (a key
  the trie genuinely lacks must still be `Ok(None)` untampered, and must NOT be called absent while
  the tree is unwalkable — an unwalkable tree cannot know what it does not contain). Gate:
  `a_single_flipped_byte_in_a_served_body_is_refused_as_a_verification_failure` — it flips a byte
  in **every blob the clean resolve actually fetched**, one at a time, taking the victim list from
  that run's own fetch log; a victim picked by position proves one path and reports on all of them.
  Mutation-checked. **The standing rule after five: when a walk can end in "nothing here", ask what
  ELSE produces that value, and give the other outcome its own type at the boundary you own.**
- **A REPUBLISH USED TO EMIT `seq 0` FOREVER, WHICH MADE THE ROLLBACK DEFENCE INERT — fixed
  2026-08-19, and the shape is worth keeping.** `PublishRootEngine::publish` derives `seq` from the
  publisher peer's own location index, and `RootProjector::new` builds a **fresh in-memory peer per
  CLI invocation** — so there was never a prior head to chain off. Measured on the shipped binary:
  two different registry contents, same key, **both `seq 0`**. `SignedSession`'s floor refuses a
  root whose `seq` went *backwards*, and two trees at zero never do, so a host holding yesterday's
  bytes could serve them indefinitely with every signature and every hash checking out — a
  withdrawn binding, a rotated target or a revocation, all re-servable. **The durable identity
  persisted the key across runs; nothing persisted the head.** Four things to know:
  **(1) read the prior head BEFORE the clean** — `run_projection` deletes `{base}/{peer}/`, which
  *is* the sequence marker, so `read_prior_head` runs first and the bytes are carried across
  (`adopt_prior_head_bytes`); the registry emitter does not clean, so it adopts inside `finish`.
  **(2) The prior SIGNATURE has to come with it**: an unchanged republish takes `publish`'s
  idempotent early return and signs nothing, which is right for a live peer whose store still holds
  the old signature and wrong for a re-projecting CLI that just deleted its own output — it failed
  with *"publisher bound no signature"* on the most ordinary republish there is. **(3) A prior head
  that is present but unreadable is an ERROR, never a silent restart** — restarting the sequence is
  indistinguishable from the rollback the sequence exists to detect. **(4) The gate needs TWO
  projectors over one directory** (`a_second_publish_into_the_same_directory_advances_the_sequence`);
  the pre-existing republish test reuses one projector, which is a live peer's shape and the case
  that always worked. Consumer half: `a_registry_that_rolls_back_to_an_earlier_publish_is_refused_mid_session`
  (v1 4 names → v2 3 names → v1 again → `seq rollback: cached 1, received 0`) — **a test that could
  not have been written before the fix**. Unchanged: across a **cold start** there is no floor, so
  the bound is still the TTL (that is what `name_resolver_max_ttl_ms` shortens).
- **THE DEPLOYMENT CAN SEED A REGISTRY PIN NOW — `/entity-deployment.json`'s `name_registry_pin`**
  (§7.4's *"preloaded Entity System Registry"*). Emitted by
  `publish --deployment-config --registry-pin=PEER_ID@ORIGIN`, carried on the durable
  `SessionConfig`, mirrored at boot to `session_config::active_registry_pin()`, read by the shell's
  `name` verb **under** the user's own `name pin`. Five things:
  **(1) It rides the DURABLE config, not the fetched doc** — a warm boot never re-fetches
  `/entity-deployment.json`, so a pin installed at fetch time is a default that works exactly once
  [AP22]; same move and same reason as `active_resolver_ceiling_ms`. **(2) It seeds, never
  overwrites**: `pinned_registry_with_source()` returns the user's pin first, and the user's is
  in-memory per tab (a reload returns to the deployment's — stated by `name pins` rather than left
  to be discovered [AP25]). **(3) An origin with no peer-id is dropped whole**, at both the parser
  and the emitter: a pin is a key, and pinning an origin would trust the origin. **(4) The emitter
  refuses a non-canonical peer-id** — stricter than `--bind`'s deliberately tolerant
  `PeerId::validate`, because a *pin* is consumed by this client, which derives the verification key
  out of the peer-id itself; the refusal is asserted on the **message**, not an exit code (the F2
  lesson). **(5) `make federation` seeds all four domains in ONE pass** via
  `entity-browser registry --peer-id`, which answers the identity question from the seed before the
  registry exists (the alternative was publishing every domain twice, or re-deriving an identity in
  bash), and the script then cross-checks the predicted pin against the id the emit published under.
  There is still exactly ONE pin, not a resolver chain — `default_rules()` remains uninstalled,
  because a catch-all needs a default registry to point at and shipping one *for everybody* is how
  two app tiers ship two. A per-deployment pin is the opposite move.
- **THE SAME-ORIGIN FEDERATION SHAPE EMITTED BARE-SLUG ORIGINS, WHICH A BROWSER RESOLVES RELATIVE
  TO THE CURRENT PATH.** `local-federation.sh` with an empty `ORIGIN_BASE` bound `foundation` /
  `registry`, so an app served at `/foundation/` fetching origin `registry` asks for
  `/foundation/registry/…` and gets a 404 that reads like a withholding origin. `expand_origin`
  treats a value as same-origin-relative **only when it starts with `/`**; anything else is a
  concrete origin. The default is `${ORIGIN_BASE:-/}` now. Note how it survived: the script's own
  comment always said `/foundation`, the code said `foundation`, and **the e2e sets an absolute
  base**, so nothing in a browser had ever consumed the same-origin shape.
- **`is_broad` IS §4.1b's GRAMMAR NOW, AND OUR FIRST VERSION WAS WRONG IN BOTH DIRECTIONS — the
  leaking one is the half arch did not name.** Until v1.19 the privacy MUST's central predicate
  (*"a pattern that matches unscoped names"*) had **no grammar**, and three implementations split.
  Arch derived one from `GUIDE-RESOLUTION` §6.2 — NARROW iff **(a)** no `*` at all · **(b)** a
  literal `@` · **(c)** the literal head before the first `*` ends in `:` · **(d)** ends in an
  **enumerated** typed suffix (§4.1b.1 — `.eth`, and that is the whole list). Otherwise broad.
  Ours asked *"does this pattern require a marker"* (`contains('@') || contains(':') || any fixed
  trailing literal`) and got **every §4.1a row right**, which is exactly why it survived. It failed
  two rows nobody had written down: `a.b` came back **broad** (harmless — it refuses config that
  should be allowed), and **`*.lab` came back NARROW**, so a chain routing every `*.lab` name to
  `did-web` would have been **accepted**. Any fixed trailing literal satisfied the old test, so
  every unreviewed namespace was narrow by default — the precise inference §4.1b.1 forbids
  (*"not inferable from the pattern's shape"*; an unrecognized suffix is **broad**, fail-safe).
  We also called `*.foo:bar` narrow, because a colon anywhere is not a `scheme:` head. **Three
  things to carry.** (1) *A POC that passes the published table is corroboration, not an answer* —
  the rows a derived grammar exists for are the ones the table does not carry. (2) **The doc
  comment was the tell**: it claimed the classifier was "deliberately conservative… calling a broad
  pattern narrow is what leaks", and the code did exactly that; a comment asserting a safety
  direction is a claim to test, not prose. (3) `TYPED_SUFFIXES` grows **only by spec revision** —
  admitting one is a privacy decision, not an implementation choice. Gate:
  `the_broad_classifier_matches_4_1b` (all six §4.1a rows + the five `REG-DISPATCH-CONFIG-REFUSED-1`
  row-7 patterns + each condition's near-miss), mutation-checked against the old classifier → red on
  `*.lab` first.
- **THE DISPATCH MATCHER IS OURS, AND `entity_registry::resolver::glob_match` IS NOT CONFORMANT.**
  `EXTENSION-REGISTRY` §4.1 v1.13 CLOSED the grammar: `*` matches any run including none, **every
  other byte is a literal** (`?` `[` `]` `\` `.` `:` `@` `/`), any number of `*`, `/` is **not** a
  separator, anchored both ends — and *"implementations MUST NOT delegate this to a path-glob or
  shell-glob library."* We were delegating to upstream's `glob_match`, which implements `?` as a
  single-char wildcard and `[a-c]`/`[!a-c]` as character classes, so **`a?c` matched `abc`** — in a
  dispatch filter that means a backend becomes eligible for names its operator never made eligible,
  with the catch-all MUST as the only thing between that and a leak. `name_dispatch::dispatch_match`
  is now ours, ~20 lines, and `glob_match` has **zero call sites** in this tree. Gate:
  `the_dispatch_grammar_is_closed` (all four `REG-DISPATCH-GRAMMAR-1` rows + the anchoring and
  escape cases + the filter itself), mutation-checked by delegating back to `glob_match` → red on
  exactly the `?` row. **The spec's own reason for spelling this out is the one to keep: a matcher
  that merely OMITS those features and one that treats them as LITERALS are indistinguishable until
  a name or a pattern carries one** — so absence of a character-class branch is not evidence.
  **Upstream is non-conformant and it is NOT ours to fix** (`extensions/registry` is neither
  `bindings/*` nor our tree): `glob_match` is called at core-rust's own §4.1 step 2 dispatch site
  (`extensions/registry/src/resolver.rs`, the `allowed` closure) and its test file *pins* the
  non-conformant behaviour (`assert!(glob_match("a?c", "abc"))`, `[a-c]x` matching `bx`). Routed in
  `ROUTING-2026-08-19-a`. **Arch's disposition said core-rust was already conformant, citing
  `matches_pattern`** — which is `core/capability`'s **path** matcher, a different subsystem for a
  different grammar, and §4.1's own paragraph says the two *"MUST NOT be read as"* each other.
- **A TABLE PIN CATCHES WHAT A PACKET UNDER-DESCRIBES — row 2 moved and nobody told us.** Arch's
  routing said *"re-pin row 6 at v1.13"*. Rows **2 and 6** had both changed: v1.12 (`6c38b00`) struck
  `did-key` and `pinned` from the shipped defaults because **neither is in §2.4.1's vocabulary**, so
  §4.2's forward-compat rule (unknown `backend_kind` → skip with a warning) was *discarding rows the
  spec recommends shipping*. `our_table_matches_the_ratified_4_1a_rows` caught row 2 on its own,
  which is the entire reason it exists — every other test there asserts a *property*, and a property
  cannot notice a token that is merely dead. `disclosure_of` is now aligned to **§2.4.1 exactly**
  (`local-name` / `out-of-band` / `self-certifying` blind; `peer-issued` blind only over a signed
  root); `pinned` and `did-key` fall to the `_` arm and are treated as **transmitting**, which is
  the fail-closed direction and matches §4.2. **Row 6 has moved three times in one day** — when a
  packet names one row of §4.1a, re-read all six.
- **THE TTL CEILING THAT PROTECTS US IS THE RESOLVER'S, NOT THE REGISTRY'S — and it is a USE bound,
  never a re-issue.** `EXTENSION-REGISTRY` §6a (1.11, arch `d1584a1`) bounds TTL on both sides, and
  only one side defends a consumer: a ceiling the **registry** enforces cannot protect anyone from
  *that* registry, which simply issues itself a long one. Ours is
  `named_site::ResolverPolicy` — `min(binding.ttl, local_max)`, computed at resolution and **never
  written back**, so `binding_hash` is the hash of the binding as published
  (`a_local_ceiling_clamps_the_lifetime_without_rewriting_the_binding` asserts the address is
  identical clamped and unclamped — if a refactor ever "helpfully" rewrote the binding to carry the
  clamped TTL, every signature over it would stop verifying). It **never extends**: a resolver
  cannot grant a binding more life than its issuer did. Both mutation-checked (`min` → identity,
  `min` → `max`). **It bites us specifically**: the `SignedSession` `seq` floor lives for the
  session, so across a **cold start** the only bound on a withheld revocation is the TTL the
  registry chose — the narrowed-not-closed half of F2 — and this is what shortens that window.
  Three things not to re-derive. **(1) `None` is the shipped default and is conformant** — §6a
  makes the ceiling a MAY and a MUST only once declared; we write no number for the reason arch
  writes none (there is no defensible constant, and picking one makes every unconfigured deployment
  *look* configured). The value is `/entity-deployment.json`'s `name_resolver_max_ttl_ms`, and
  **zero is dropped, not honored** — honored literally it expires every binding instantly and the
  operator sees "no binding for this name", indistinguishable from a bad signature or a revocation.
  Same failure shape as emitting `ttl` in seconds. **(2) It rides the DURABLE config, not the
  fetched deployment doc** — a returning profile never re-fetches that doc (persisted > fetched,
  D16), so a ceiling read at fetch time would apply on a cold boot and silently not on a warm one:
  the security-half-nobody-can-reach shape [AP22]. It goes through `SessionConfig` → mirrored to
  `session_config::active_resolver_ceiling_ms()` at boot, the same move `boot_fast_paint`'s
  localStorage mirror already makes and for the same reason (the shell's `name` verb resolves
  inside a `spawn_task` holding no config handle). Known limit, inherited not new: a profile that
  already persisted a config keeps its old ceiling until that config is refreshed. **(3) `policy` is
  a REQUIRED argument to `resolve_name`, deliberately** — an overload that defaults it is how the
  protecting half gets skipped at the one call site that needed it. Any new resolution surface
  passes one. And **say it when you clamp** [AP25]: the shell prints issued-vs-honored, because a
  silent clamp makes a correctly-issued binding look like a registry that mis-set its TTL.
- **A registry binding's target must BE a peer-id — refused at the emitter [F9].** Nothing checked
  it, so a typo became a *signed* binding: `--bind=example.test=foundation@…` (a directory slug
  where the id belonged) emitted happily, and the only symptom arrived at the far end of the chain
  on a **consumer's** machine as *"the named peer-id carries no public key"*. The operator, the one
  person who could fix it, saw a success message. The bar is `PeerId::validate` — well-formed in
  **either** Ed25519 form — deliberately not `from_peer_id`: refusing a legacy-form target would be
  our emitter deciding what *other* consumers may resolve. Same posture as D10: refuse before
  anything is written, name every offender. Found by mistyping a shell pipeline, which is the
  argument for running the thing over re-reading it. **The fixtures said `"2PEERTARGET"`, which is
  not a peer-id in any form** — a fixture that could not exist in production was hiding this.
- **ENUMERATION IS SAFE NOW — `SignedSession::enumerate`, and the entry below saying we had no
  decoder is CLOSED.** A signed root's key set is recoverable (`EXTENSION-TREE` §3.1's leaf is
  `[key, value_hash]`), and the two things that used to block a browse surface are both gone.
  Five things:
  - **Do NOT reach for upstream `collect_all_bindings`.** It is the obvious call and it is the
    silently-short walk — a missing `Entry::Link` skipped with a bare `if let Some(..)`, returning
    a `BTreeMap` with nowhere to report the miss (measured: 1 name of 24 hidden, root hash still
    verifying). Ours walks the HAMT itself and **fails** on a declared child it cannot fetch, which
    is arch's D9 for the one operation that did not satisfy it. Resolution has been conformant for
    a while; **enumeration was the genuinely open half.**
  - **A node that does not DECODE is a failure, not an empty node.** Treating an undecodable node
    as childless is precisely how a walk shortens without failing — the same move as calling an
    unwalkable tree `Absent`. Same for a node of the wrong entity type.
  - **It rides the same pump, so it inherits the defences rather than opening a door around
    them**: `fetch_root` enforces the `seq` floor, and every node is `verify_and_decode`'d on the
    way into the cache, so a tampered interior node is a verification failure and not a shorter
    answer.
  - **`entity-tree` moved from native-only to a shared dependency**, and the old comment
    ("the browser consumes rather than emits") was true only while consuming meant resolving ONE
    key. The keys live *inside* the nodes, so a consumer that will not decode a node cannot answer
    "what does this registry carry" from the signed root at all — which is exactly why that
    question used to get answered from a host-served `.list` that commits to nothing. Pure codec,
    builds for wasm32 unchanged.
  - **What it does NOT claim**: it is authoritative about what *this root commits to*, not about
    what the publisher knows. That is the honest bound, and it is strictly stronger than the
    `.list` artifacts. A browse surface may now show a signed listing; it still must resolve
    anything it is about to act on.
- **THERE ARE TWO CONSUMERS OF A FOREIGN TREE AND THEY DO NOT SHARE A TRUST MODEL — the naming
  chain is a SPIKE, the Site Browser is the PRODUCT, and nothing wires them together.** Measured
  2026-08-20; the guide has it as `GUIDE-PUBLISHING-AND-NAMES` §7, which is the canonical home. The
  `name` verb pins a **publisher key**, walks two signed roots, holds a `seq` floor — and then
  **prints 160 bytes of preview into the scrollback and navigates nothing**. The Site Browser, which
  is what a user actually browses with, reaches a foreign peer over `http_poll`'s two-hop
  (`path → .bin pointer → content → hash-verify`) against an origin seeded from
  `entity-deployment.json`. `resolve_name` has exactly **one** caller in `src/`; `session_cache` and
  `signed_fetch` have **no call site on the Site Browser's fetch path**. Four things to carry:
  - **The two-hop check is real but cannot catch a lying ORIGIN, because the origin supplied the
    pointer too.** It proves the body is the canonical pre-image of the hash it was fetched under —
    a genuine gate against corruption and truncation. Serve a different hash at
    `sites/x/pages/index.bin` plus a matching body, and every check passes. That is precisely the
    hole the signed root closes, and this path does not read one. **Do not describe the Site
    Browser's fetch as "hash-verified, therefore safe"** — the sentence is true and the inference
    is not.
  - **It is defensible today ONLY because of who supplies the origins**: the deployment's own
    document, so the visitor trusts the site they already loaded the app from and no more. It stops
    being defensible the moment a foreign origin can arrive from anywhere else — a resolved name, a
    typed URL, a link from another publisher. **Whichever of those ships first makes wiring the
    signed path in a prerequisite, not an improvement.**
  - **PUBLISHING ONE SITE UNDER TWO PEERS IS SAFE IN THE DATA AND AMBIGUOUS ON THE SCREEN — measured
    2026-08-24, because a consolidation was being considered.** Question: if the same site id is
    published under two identities (e.g. everything re-published under one domain's key *and* still
    live under its own), does the Site Browser conflict? **No conflict, no corruption, no
    shadowing — you get two rows.** Every layer is keyed by the pair:
    - `site_directory` dedups on **`(peer, site)`** (`seen.insert((r.peer, r.site))`), so both
      survive by construction — neither hides the other.
    - Sort is `bookmarked → owned → site → peer`, so the twins land **adjacent**. That is the good
      case: the ambiguity is at least visible in one glance instead of scattered.
    - Prefs and provenance are keyed `(site_peer, site)` (`prefs.rs`, `app_paths::site_cache_prefs_path`),
      so bookmarks, keep-offline and visit counts **do not bleed** between them.
    - `is_current` compares **both** (`r.site == cur.site_id && r.peer == cur_peer`), so the
      highlight cannot attach to the wrong twin, and a cached row's click sends the **concrete
      foreign peer id** (`bookkeeping_peer`), so you open the one you pointed at.
    - Underneath, content is content-addressed (identical bytes dedupe, different bytes get
      different addresses — there is no key to collide on) and tree paths are peer-namespaced
      `/{peer}/sites/{site}/…`.
    **The cost is entirely presentational, and it is this file's own complaint one turn worse:** the
    two rows carry the same title, and **the only thing distinguishing them is the host** — a
    transport fact — on rows where `verified_at` is `None` for both. So a consolidation manufactures
    exactly the ambiguity B-3 exists to remove, at the one place the UI cannot currently answer.
    **And there is no freshness signal to break the tie:** `last_reconciled` is on `SiteEntry` and is
    **not rendered anywhere** in the rail (grep `dom/site_directory.rs` — zero hits), so if one copy
    goes stale the screen says nothing about which is current. **Recommendation, and it agrees with
    the standing `cgid-10-235` decision (*one domain = one build = one identity = one bucket*): do
    not consolidate.** The registry is what replaced the need for it — cross-domain discovery without
    copying — and the authorship point is sharper now that a binding *signs* `name → peer-id`:
    serving another domain's content under this domain's key is that key claiming authorship of it.
    `cached · {host}` (`dom/site_directory.rs::subline`); the page chrome says only the site title,
    so a cross-peer `entity://` link changes publisher **silently**. Host is a transport fact,
    key is an identity fact, and only the second is what the trust chain makes answerable.
  - **DO NOT WIRE `name open` INTO THE SITE BROWSER'S NAVIGATE PATH — it is the cheapest commit on
    the board and it is a D2 violation.** The two halves are one call apart in the same binary: the
    shell's `name open` resolves, walks **both** signed roots, holds a `seq` floor and reports
    `"{key} verified — N bytes"`; the Site Browser fetches over `http_poll`'s two-hop and can only
    ever render `not verified`. *"Make `name open` open the site"* looks like plumbing and reads as
    an improvement in review. Done naively it inherits the **Site Browser's** trust model, so a
    key-pinned, signed-root-verified name renders under chrome that says `not verified` — or worse,
    gets wired to say `verified` on the strength of the **resolution** while the page bytes came down
    the **unverified** path. That second one is `EXTENSION-NETWORK` D2's violation arriving as a UI
    convenience. **The trigger for the trust work is not "a new way to name a site" — it is ANY path
    by which bytes reach the renderer from an origin the deployment did not supply.** Wiring it is
    fine *after* `SignedSession` is threaded through the navigate path; it is not fine as the wire
    alone. (arch R-21 / `PROPOSAL-CONSUMER-TRUST-ANCHOR-ON-THE-STATIC-PATH` D2/D3, confirmed by us in
    `ROUTING-2026-08-20-g` §3 — where we also asked arch to state the prerequisite in terms of the
    navigation path rather than the feature, because the feature framing does not catch this case.)
  - **~~A link alone does not reach a stranger… there is no affordance anywhere in the app to add an
    origin… the origin registry is deployment-owned.~~ FALSE SINCE 2026-08-24 — WE SHIPPED THE
    AFFORDANCE AND THIS PARAGRAPH IS THE THING IT INVALIDATED.** The mechanism half still holds
    (`entity://{peer}/…` needs a registered origin or it is `ResolveError::Unreachable`), but the
    conclusion is spent: `RegistryBrowser::open_in_site_browser` calls
    `content_site::origins::set_origin` under the **system** peer and then warms that publisher's
    sites. So *"how do I visit somebody else's site"* has a second answer now — **resolve a name**,
    and the origin arrives from a signed binding rather than from `/entity-deployment.json`.
    **This is the exact shape this file keeps recording: a "we cannot do X" line whose expiry is
    somebody shipping X** — and it went stale *inside the same repo, four commits after the sentence
    was last read*. Its neighbour above (the B-3 trigger) is the rule that fires on it; see the
    entry below.
  - **THE B-3 TRIGGER HAS FIRED, BY THIS FILE'S OWN CRITERION — flagged 2026-08-24 and confirmed
    here against the code.** The rule three bullets up says the trigger is
    *"ANY path by which bytes reach the renderer from an origin the deployment did not supply"*.
    A registry-resolved origin is exactly that, and it is live. Four things, and the third is the one
    that keeps the severity honest:
    - **What did NOT happen, and it is what the rule was most afraid of:** nothing got wired to claim
      `verified`. `verified_at` is still hard `None` with the reason inline, and the rail still
      renders `not verified` on every foreign row. The D2 violation the bullet warns about — a
      verified *resolution* laundering an unverified *fetch* — has not shipped.
    - **The exposure did not change in KIND; the ORIGIN SET widened.** Before: origins came from
      `/entity-deployment.json`, i.e. whatever host served the app. Now: that list **plus anything
      the pinned registry names**. The gap is identical in both — the two-hop check proves a body
      matches a pointer *the same origin served*, so it is real against corruption and silent about
      authorship. What moved is **whose judgement bounds the origin set**: it was the deployment
      operator, and it is now also the **registry operator**. Today those are the same people.
    - **A registry-resolved origin is not an arbitrary one, and calling the trigger a "prerequisite"
      without saying so overstates it.** That origin arrives inside a binding that was signature-
      checked through the registry's signed root, with the D1 name check, a non-null TTL and the
      §6a.6 revocation probe. It has *better* provenance than a deployment-config origin, which is
      an unsigned JSON file. The residual risk is narrow and worth stating in one sentence: **a host
      named by a legitimate signed binding serves bytes the publisher never signed.** That needs the
      *named* host to be hostile or compromised — not any attacker.
    - **So the honest status is: the condition fired, the labelling is still honest, and B-3 is now
      load-bearing rather than optional.** Do not read this as "ship B-3 before the release"; read it
      as "the argument that made the gap defensible — *the deployment decided every origin in
      advance* — is no longer available, and the replacement argument is trust in the registry
      operator." When that stops being us, this stops being a deferral.
- **`--plan` ACCOUNTED FOR SITES WHILE THE SAME CLEAN DELETED `{peer}/apps/**` — the gate said GO
  and 35 objects went.** Reported from a full local dry run of the publish path, and it is the sharpest shape of the
  operator-surface bug above: not silence, but **active reassurance**. The measured sequence:
  publish with `--ingest-apps` (304 objects, 35 under `apps/`) → plan the same publish *without* it
  → *"1 present, 1 incoming — 1 kept, 0 added, 0 REMOVED · nothing would be removed"*, **exit 0** →
  run it → `apps/` 35 → 0. In production that is *push a content fix, forget one flag, every game
  disappears from the live domain*, with the safety check having just blessed it. Four things:
  - **The site accounting was never wrong.** `apps/` simply was not in it — which is why nothing
    looked broken and why reading the plan's output could not have caught it. **A plan must cover
    every namespace its publish REPLACES, not the one it was written for**; the clean is wholesale,
    so anything it can delete is in scope by construction. A third subgraph under the peer prefix
    owes a term in `run_plan` in the same commit that adds it.
  - **The normal path warns too, independently of `--plan`** (`warn_replaced_app_sets`, sharing
    `dropped_app_sets` with the plan for the same reason the sites pair share theirs — a plan that
    disagrees with the publish it predicts is worse than no plan). The plan is the gate you run
    deliberately; this is the one that fires on the run you did not think needed a gate, which is
    exactly the run that drops the flag.
  - **`--html-only` is exempt and its test says so**: it never cleans `{peer}/`, so claiming it
    would remove apps is a false destructive exit on the one safe mode — and a gate that cries wolf
    gets routed around with `|| true`.
  - **The regression test ends by RUNNING the destructive publish**, not just checking the exit
    code. Otherwise it would still pass if the publish quietly stopped cleaning apps — a different
    bug wearing this one's clothes — and the plan would be warning about nothing.
- **A `--verify` that finds nothing now names the identity the tree IS under** (handoff §4).
  *"NOTHING WAS VERIFIED … this is not a clean tree, it is an absent one"* is accurate and still
  reads as *"my tree is corrupt"* when the truth is *"I resolved a different identity"* — one flag
  away (`DEMO_IDENTITY=1`, `--identity-seed=`, or the standing trap that `make site` and a host-run
  `entity-browser publish` resolve different `ENTITY_DATA_DIR`s). The evidence is in the directory:
  a publish leaves `sites/{peer}/`, so the other identity is *visible from here*. `other_identity_hint`
  **returns its message as a value** and is asserted on the **message**, because pointing verify at
  the wrong identity and pointing it at an empty directory both exit 1 — the same lesson
  `parse_registry_args` was rewritten for, and the reason an exit-code test cannot tell a good
  refusal from a misleading one.
- **The full naming path is one call now — and a peer-id IS the pin.**
  `src/content_site/named_site.rs::resolve_name` walks a pinned registry's **signed root** to
  `binding/by-name/{name}`, checks `binding.name` (arch D1), refuses a null TTL (D3), probes the
  §6a.6 revocation key inside the signed tree, and hands back a peer-id plus `NameEvidence`
  saying what was checked. `make federation` stands the whole thing up locally (N domains + a
  registry, `--verify`d, with a `MAPPING.txt`); the `DOMAINS` table in `tools/local-federation.sh`
  IS the seeding. Three things to know: **(1) a registry binding names a peer-id, not a key, and
  that is not a key-distribution problem** — for Ed25519 canonical form the peer-id *embeds* the
  32-byte public key (`PeerId::derive_public_key`, `hash_type = identity`), so
  `PinnedPublisher::from_peer_id` decodes the pin straight out of it; it returns `None` for the
  SHA-256 legacy form, which genuinely needs an out-of-band key. **(2) Resolving over the signed
  root closes our routed F1** — `peer_issued::resolve_one` trusts the host-served by-name
  pointer, and a host that repoints one file gets a different domain with the registry's own
  trust anchor; walking the root removes the choice. Do the name comparison anyway. **(3) F2 is
  NARROWED, not closed** — a host cannot drop a revocation out of a root it serves, but it can
  serve an *older* root, which only the session `seq` floor catches, and only within a session.
  Across a cold start the bound is the TTL. Don't record this as "revocation withholding is
  fixed".
- **A signed root IS enumerable — but the walk that enumerates it is not the walk we built, and a
  withheld node shortens it SILENTLY.** Two separate facts, and mixing them up cost us a filed
  finding (F4, refuted by arch `1782d6d`; measured back in
  `STATUS-2026-08-18-a-a-signed-root-enumerates-but-a-withheld-node-is-silent`). **(1) The keys are
  in the nodes.** `EXTENSION-TREE` §3.1's leaf is `[key, value_hash]`, and upstream
  `entity_tree::trie::collect_all_bindings` (§3.5's `walk_entry_collect_bindings`) returns the whole
  reachable key set — so *"what names does this registry carry"* is answerable from the signed root
  alone, with no new mechanism. Publish at `prefix: system/registry/binding/by-name/` and the trie's
  key set IS the name set. **Do not reach for `signed_root::trie_closure` for this**: it scavenges
  33-byte windows for *hashes* and is structure-agnostic on purpose (a publisher must not need the
  HAMT encoding to project it), so it recovers the closure and **no keys at all**. We reasoned about
  what was recoverable from outside the node *because* we had deliberately avoided looking inside
  it, and concluded the key set was gone. Check the decoder next door before filing a spec finding
  about recoverability. **(2) `collect_bindings_into` skips a missing `Entry::Link` with a bare
  `if let Some(..)` and returns a `BTreeMap`, not a `Result`** — so a hostile origin withholding one
  interior node hides names with **no error, nowhere to report one, and a signature that still
  verifies** (the root hash is intact). Measured: 1 of 24 names hidden, silently
  (`withholding_a_trie_node_shortens_the_walk_silently`). The hidden name stays *resolvable* by
  anyone who already knows it, so the browse and the resolve disagree and neither complains. **Any
  registry-browser surface must therefore treat an unresolvable link as a FAILURE, not as the end of
  a branch** — arch's D9 says a walk cannot be shortened without failing, and that is a property of
  the spec, not of the code that exists today. This was the third appearance of the same seam
  (B14's `Ok(None)` for an unwalkable tree; `--verify` reporting *16 pointers verify, 0 broken*
  over an incomplete closure) — and there is now a **fourth**, the fix for that third one, which
  could not fire (see the `--verify` entry above, F8): **"absent" and "withheld" keep arriving as
  the same value, and each fix was to make incompleteness an error rather than a shorter answer.**
  **Do not repeat the reason `list_names`' doc gave for this** — it said a signed root cannot
  answer *"what keys exist"*, which is the refuted claim, restated inside the module that had the
  correction next door. The true reasons are narrower and both survive: our browser-side reader has
  no trie decoder (buildable, not built), and the upstream walk is itself silently short.
- **THE NAMING FLOW, so nobody re-derives it: three peer-ids, and the user's is not one of the two
  that matter.** A **publisher** identity (per domain) signs that domain's content root; a
  **registry** identity — deliberately a *different* key — signs `name → peer-id` bindings; and the
  **user's browser peer** owns the local tree and is **involved in none of it**. It is not
  registered, nothing is published under it, no signature mentions it. Resolve is two hops from
  **one** pinned string: walk the registry's signed root to a binding (checking `binding.name` D1,
  non-null TTL D3, and the §6a.6 revocation key *inside* the signed tree), then pin the peer-id it
  named at the origin its `transports` carried and walk **that** peer's signed root to the page. The
  pinned string is a peer-id, not a key, because for Ed25519 canonical form the peer-id **embeds**
  the 32 public-key bytes — so nothing is fetched to learn who the registry is, and the origin is
  trusted for nothing at either hop. Full write-up:
  `STATUS-2026-08-18-b-the-naming-flow-end-to-end-and-a-browser-can-now-drive-it`.
  **The user-facing home for all of this is
  `docs/architecture/guides/GUIDE-PUBLISHING-AND-NAMES.md`** — the three publish routes (site,
  registry, wasm app), what a consumer verifies at each hop, the serve requirements (CORS +
  mutability-keyed cache headers, measured), and a table of what is NOT closed. Point publishers
  and ecosystem implementers there rather than at a status doc; keep it current when this arc
  moves, and let `GUIDE-DEPLOYMENT-AND-CONFIGURATION.md` stay authoritative for the `make site`
  knob table (the guide cross-references it rather than restating it).
- **A `SignedSession` must outlive the call, and it is keyed on the PUBLISHER — never on
  `(publisher, origin)`.** `src/content_site/session_cache.rs`. The `seq` floor lives in the session,
  so a client per fetch lets an origin roll a site back *page by page* while every signature and hash
  still verifies; and keying on the origin hands a hostile **mirror** a fresh floor of zero by
  serving the same tree from a second URL — the same rollback wearing a hostname. One session per
  publisher, process-wide (`Rc` + `thread_local`; the browser arm is single-threaded and
  `SignedSession` is `!Send`, so `Arc<Mutex<…>>` buys nothing and will not compile against the
  `!Send` futures). **Known limit, recorded not guessed: the first origin wins** — a publisher that
  genuinely moves needs the origin threaded through `PinnedPublisher`, which is a refactor of the
  trust chain and was not done. Any new surface that reads a published tree must go through
  `session_for`, or it gets its own floor and the rollback comes back.
- **The naming chain has a browser surface now — the `name` shell verb — and before it,
  `resolve_name` had ZERO callers.** Everything under it compiled to wasm and nothing called it, so
  "the browser can resolve a name" was true of the code and false of the app. `name pin <registry-pid>
  <origin>` / `name resolve <name>` / `name open <name> [site] [page]` / `name pins`. **Browser-only
  by construction**: the fetch is `window.fetch` and a `BinSource` future is `!Send` while native
  `spawn_task` requires `Send` — so the native stub says so rather than pretending. There is
  **no default registry**, deliberately: the chain is B16b, held on arch's `name_format_dispatch`
  ruling, and a pin is explicit and fail-closed until then. When wiring a name-bar or the site
  window, extend this verb's path rather than re-deriving the two hops.
- **`name_format_dispatch`: the catch-all is safe or not depending on the BACKEND, not the glob —
  and our resolve never puts the name on the wire.** `src/content_site/name_dispatch.rs`, proposed in
  `PROPOSAL-NAME-FORMAT-DISPATCH-DEFAULTS-AND-THE-NAME-BLIND-BACKEND.md` (ours to draft, arch's to
  ratify). Two landed statements point opposite ways: `GUIDE-RESOLUTION` §6.1 wants bare `alice` to
  route via `*` to *"local-name, then default registry"*, while `EXTENSION-REGISTRY` §4.1 step 2
  calls a broad-matching backend *the* way a private name leaks. **The hidden variable is that
  disclosure is a property of the backend.** Measured, not argued: our `resolve_name` walks a signed
  root, so every request is `content/{aa}/{bb}/{hex}` and the key is matched inside a node already
  fetched — a *hit* never sends the name, and a *miss* on a name the registry does not carry costs
  **4 requests, none carrying the name** (`resolving_a_name_never_puts_that_name_on_the_wire`,
  `a_name_the_registry_does_not_carry_dies_in_the_trie_without_being_sent`). So `peer-issued` over a
  signed root is **name-blind** and may sit in the catch-all; `dns-txt`/`well-known-url`/`did-web`
  **transmit** the name and must require an explicit `@`/`scheme:`/suffix marker. Three rules that
  cost a design pass each: **(1) an unclassified backend kind is TRANSMITTING** — fail closed, same
  posture as "no signaling node, no establisher"; **(2) §4.1 step 2 is a FILTER, not a routing
  table** — a name matching several patterns is eligible at the *union*, and "ordered glob list,
  first match wins" is the obvious reading and the wrong one (pinned by
  `matching_several_rules_yields_the_union_not_the_first`). **This is now the spec's reading —
  arch WITHDREW its opposite ruling** (`ROUTING-2026-08-18-j` §1; they had folded first-match-wins
  16 minutes after we pushed the proposal, reading our tree at the commit *before* it). The
  mechanism was never in doubt and never new: `resolver_chain[].priority` decides **who answers**,
  `name_format_dispatch` decides **who is eligible**; a second ordering on the filter gave the
  resolver two rules that disagreed. §4.1a's *"rule 4 precedes rule 5"* is gone with it, and
  `GUIDE-RESOLUTION` §6.1 was corrected to the spec — **guides are convenience, not authority.**
  **D-A/D-B is CLOSED, in our favour — REGISTRY 1.7 → 1.8, arch `8bfc9b6`.** §4.1 step 2 is re-keyed
  from **remoteness** to **name transmission**, and `peer-issued` *resolved per §6a.4 through the
  signed root* is now an explicit **MAY** in the catch-all; §4.1a row 6 is
  `["local-name", "pinned", "peer-issued"]`. What decided it is the half we argued and they
  verified: §6a.4 makes verification *inside the signed tree* mandatory and §6a.3a bars the
  host-served listing from being authoritative, so **the kind fixes the mechanism** and the rule can
  key on disclosure without becoming unverifiable. They did **not** adopt D-A's *"nor a reversible
  function of it"* wording, correctly — it is falsifiable on our own hit path (the binding blob's
  hash identifies the name to the origin that published it); the spec states the residual instead
  (hash-prefix oracle on a miss, a public binding's blob on a hit, never the queried string).
  *Historical, and the reason the entry above still matters:* this was ruled **against** us first, in
  `096fa96` (**08:45**, sixteen minutes after our proposal at `cb6d063` **08:29**), while arch's own
  `-j` §1 and §5 twice told us it was *"still open and still yours"* — their packet and their spec
  disagreed, and the spec is what engines build against. **Verify a spec claim in `specs/`, not in
  the packet that describes it** — the same rule as "read the resolver, read the caller", applied to
  prose. Our objection, filed in `ROUTING-2026-08-18-k` §2.1 with the measurements: the rule was
  written per **backend kind**, and
  disclosure is a property of the **resolution mechanism** — the same `peer-issued` backend leaks
  the name over a host-served `by-name` pointer (it goes in a URL path) and leaks nothing over a
  signed root. Their strongest counter, which we made for them: name-blindness is an implementation
  property and is not wire-checkable, so a categorical ban is the conservative default — in which
  case the spec should *say that* rather than assert a disclosure claim that is false for what we
  ship — and that is the shape they adopted. **(3) The name-blindness is conditional
  on the signed-root walk** — a consumer trusting the host-served `by-name` pointer puts the name in
  a URL path and loses it, which is why arch's D1 is also a *privacy* prerequisite, not only an
  integrity one. **`default_rules()` is STILL not installed anywhere, and ratification did not
  change that** — the blocker was never only the ruling, it is that there is **no default registry
  to point a catch-all at**, and shipping one before that exists is how two app tiers ship two.
  B16b's surface stays fail-closed (registry added explicitly, every entry carrying a non-`*`
  pattern). **The "sync row 6 to `["local-name", "pinned", "peer-issued"]`" instruction that stood
  here is WITHDRAWN — following it would now be a regression.** v1.12 (`6c38b00`) struck `pinned`
  as a **category error**: §4.1 step 1 returns a pinned match *before* the step-2 filter runs and
  §4.1.2 uses `pinned` as a **`backend_id`** on the synthesized result, so it is a result label and
  not a dispatch target — no configuration could act on it, and §4.2's forward-compat rule would
  discard the token with a warning. **Verified against arch HEAD `05faaa5` (v1.18): row 6 is
  `["local-name", "self-certifying", "out-of-band", "peer-issued"]`, and `default_rules()` already
  matches all six rows exactly** (`our_table_matches_the_ratified_4_1a_rows`, green). This entry is
  the reason that test exists — **row 6 has now moved four times, and this is the first time the
  stale copy was in our own instructions rather than in arch's.** When a packet names one row of
  §4.1a, re-read all six *in `specs/`*, and re-read this paragraph before acting on it.
- **`name@X` is ONE separator, decided by decoding X — and a charset test is the wrong
  discriminator.** `split_qualified` (D-D): X that passes `PeerId::validate` is a **verification
  pin** (resolve however the chain does, the answer MUST equal X); anything else is an **authority
  handle** (resolve *at* it). Do not reach for "is it Base58?" — a handle like `entitychurch` is
  entirely within the Base58 alphabet and is still a handle
  (`a_base58_alphabet_handle_is_still_a_handle`); only a full multikey decode separates them. The
  residual collision is closed **at issuance** (`handle_is_issuable`), where a human reads the
  error, not at parse where nobody sees it. Split on the **last** `@` so a local part containing one
  survives.
- **A `peer-issued` binding MUST carry non-empty `transports` (arch D10) — refused at the emitter,
  tolerated at the resolver.** Our F6, confirmed: §4.1.2 promises a §6.5 transport-resolution
  fall-through, and §6.5.4 makes profile discovery out-of-band in v1 — so for a *statically
  published* target there is nothing to fall through to, and a transportless binding resolves to a
  peer-id the consumer cannot reach. `emit_registry` refuses it before anything is written and names
  every offender, not the first; we used to emit it and print a note, and **a note is not a bound**.
  The §4.1.2 **pin** carve-out is untouched — a pin is the user's own assertion, so reaching the peer
  is the user's problem. The **resolver stays tolerant** (`http_poll_origin(&[])` → `None`, recorded
  rather than guessed): D10 binds issuers, and a consumer meets registries it did not emit.
- **Emitter and resolver must share ONE normalization, from the same function.** The registry
  emitter briefly lowercased names while `peer_issued::resolve_one` calls
  `normalize_name(name, "none")` — NFC, **no case folding** — so an operator typing
  `Foundation.Example` would have published a key the resolver could never build. Both ends call
  the same upstream function now. Re-deriving "what normalization probably means" on one side is
  how the two ends drift apart silently.

## Content sites & documents

- **A `format:html` PAGE IS A DOCUMENT IN A SANDBOX, NOT MARKUP — and "there is no sanitizer" no
  longer means "no HTML".** F-CONTENT-1 downgraded `format:html` to escaped text because the only
  mount we had was `set_inner_html`. It now renders in an `<iframe sandbox="">` — the **empty**
  string, every restriction on — which is what carries a pre-rendered Pandoc paper/book from the
  papers pipeline. Six things earned on it:
  - **The two outcomes have different TYPES, and that is the enforcement point.** `render_page`
    returns `PageRender::Markup | ::Document`, never a `String`; the `String`-returning helper was
    **deleted** rather than kept, because a bare `String` is exactly what someone grabs and
    `set_inner_html`s. This is the *"give the two outcomes different types"* move from the
    absent-vs-withheld seam, applied **before** the bug instead of after the fourth one.
  - **`sandbox=""` is a TIER, not an oversight.** We now run three: `""` (passive document —
    nothing executes), `"allow-scripts"` (`dom::games` app bundle, opaque origin), and
    `"allow-scripts allow-same-origin"` (an L5 app, our own payload). Adding a token to the first
    silently promotes **every publisher on the network** to the app tier. It is also *stronger*
    than gate G2 asks for: §8 wants an audited allowlist sanitizer and itself flags completeness as
    the fragile part — nothing executing makes completeness moot. Fail closed: anything not
    **exactly** `html` takes the escaping path, so `"HTML"`/`"text/html"` are markdown.
  - **THE CHEAP ASSERTION SHADOWED THE REAL ONE, AND ONLY MUTATION FOUND IT.** Phase 19-doc first
    compared the sandbox *attribute* before entering the frame to check that a script actually
    stayed inert. Mutating the tier to `allow-scripts` went red — **on the attribute** — so the
    behavioural assertion was never reached and had never been seen red. Reordered (property first,
    spelling last) it fails on `SCRIPT RAN`, as intended. Generalize it: **when a gate has a cheap
    check and an expensive one for the same property, the cheap one must come second**, or
    mutation-checking proves only that the cheap one works. A gate never seen red is not a gate —
    and neither is an assertion that a *sibling* assertion always fires before.
  - **`make test` compiles the e2e to NOTHING** (`#![cfg(feature = "e2e")]`), so "the suite is
    green" is not evidence the e2e file even parses. This seat concluded a new phase compiled off a
    cached `make test-one` that had rebuilt nothing; the real `make e2e-worker` then failed on a
    wrong `enter_frame` signature. Only a run with `--features e2e` compiles it.
  - **A document's colours are CONTENT, not theming** — they render in an opaque origin our
    `--site-*` tokens cannot reach. Bundled demo bodies live in `views/content_site/demo_content.rs`,
    exempt in `tools/ui-lint.sh` **by name** and `i18n-ignore-file`d (the browser does not translate
    the pages it renders). Extracting them ratcheted the raw-hex baseline **7 → 0** in the same
    commit. Don't put anything that renders through our own DOM in an exempt file.
  - **The frame carries no border, radius, background or token, deliberately** — themed chrome
    around unthemed content advertises a relationship that does not exist. And a document already
    has its own column width, so the pane goes full-bleed (`.cs-pane-doc`): nesting a 42rem document
    inside our 720px reading column squeezes it twice, which is what the first build looked like.
  - **THE DOCUMENT IS DELIVERED FROM ITS OWN `blob:` URL AT `sandbox="allow-same-origin"`, BECAUSE
    `srcdoc` BROKE EVERY ANCHOR AND `data:` WENT BLANK ON EVERY REAL BOOK.** A `srcdoc` document
    inherits the **parent's** base URL, so `href="#section"` resolved to `…/index.html#section` and
    the frame **navigated away from the paper to our own host page**. A Pandoc paper's entire
    navigation is anchors (the Contents list; every chapter jump in a one-file book), so books
    rendered beautifully and could not be read past the first screen. Things to know:
    - **`data:` SHIPPED FIRST AND WAS WRONG, and the reason is the sharpest lesson here: CHROME
      CAPS A `data:` URL AT 2 MiB, SILENTLY.** Bisected — **1,398,856 bytes renders, 2,098,624
      does not** — and past the cap the frame renders *nothing*, raises *nothing*, logs *nothing*.
      The published corpus book is **7.9 MB (10.9 MB base64)** and the figure-heavy single papers
      (06, 11, 12) are over the cap too, so **every artifact this tier exists to carry was blank in
      one of our two engines** while the gate's 5 KB demo document stayed green all day. Measured
      on the REAL book across Firefox 149 and Chrome: `srcdoc`+`""` destroys the document;
      `blob:`+`""` is inert; `data:`+`""` works small and is **BLANK in Chrome** at book size;
      **`blob:`+`allow-same-origin` renders in both**; an `http` URL at `""` renders in both.
      **A ceiling only the real payload crosses cannot be caught by a fixture that never
      approaches it** — the fixture is the thing that was too small, not the assertion.
    - **`allow-same-origin` ALONE is not the app tier, and the token that must never join it is
      `allow-scripts`.** Alone it grants an origin to a document with no way to *use* one — nothing
      executes, so nothing can read a cookie, touch storage, or reach `parent.document`. Together
      the two are full origin access for every publisher on the network. This tier is now **one
      token from the app tier in each direction**, which is why Phase 19-doc compares the whole
      attribute with `==` (a `contains` would let a second token through) and why the mutation to
      `sandbox=""` is worth keeping: it goes red on **behaviour** (`hash: ""`, the anchor inert),
      not on spelling.
    - **REVOKING ON THE FRAME'S `load` KILLED EVERY BOOK A FEW CHAPTERS IN — the entry that stood
      here saying "fragment navigation still works after the URL is revoked" was measured on ONE
      click, and one click is about how long it holds.** The URL is now retired on **replacement**
      (`retire_live_document_url`, `dom/content_site.rs`): exactly one object URL is live at a time,
      and the next document mount retires the last. The D9 accounting the revoke existed for is
      unchanged — it was never "one per rebuild"; it is "one, ever" — and the reader gets a document
      whose own table of contents keeps working. Measured in an isolated page with **trusted**
      WebDriver clicks, only the revoke varying: **revoke-on-load 7/12 jumps, revoke-on-replacement
      12/12**, and that split holds at **every** size tried — 21 KB, 1.2 MB, and all seven real books
      (0.81 → 7.82 MB). So this one is *not* the fixture-size shape the `data:` ceiling was.
      Three riders, each of which cost something:
      - **The second retire site is the trap.** The first fix ALSO retired when the surface rendered
        markup — "the reader left the document, drop the book" — which is wrong, because this surface
        renders markup **between** document renders. That retire revoked the URL of the document
        still on screen, so the bug survived its own fix and looked identical. Only tracing both
        sites showed it: `mount A · retire A · mount B · retire B`. **A document URL is retired by
        the NEXT document mount and by nothing else.**
      - **Phase 19-doc does NOT catch this, and the note there says so.** Mutation-checked: reinstate
        the original revoke and the phase stays green 8/8 with `history.length` climbing. A cruder
        mutation (revoke *before* load) does go red — on the earlier "did it render at all"
        assertion, so it never reaches the loop either. **Why the harness is blind is unidentified**;
        it is not size. Until someone finds it, a green 19-doc is not evidence about repeated
        navigation, and the check that works is serving a real book and clicking its TOC a dozen
        times.
      - Never `Closure::forget` here: that would trade a bounded blob leak for an unbounded closure
        one. (The `load` listener is gone entirely now, so there is nothing to forget.)
    - **The blob MUST carry `type: "text/html"`** (`BlobPropertyBag`, a Cargo feature this needed
      adding). Without it the frame renders the book as plain text — which reads as a defect in the
      document rather than a missing property bag in our mount.
    - **THE END STATE THAT REMOVES THE TRADE-OFF IS A REAL URL.** Measured: an `http` same-origin
      URL at `sandbox=""` renders the 7.9 MB book in both engines *and* keeps its anchors — no
      ceiling, no token. It needs a service-worker (or Tauri asset-protocol) route, a cache
      lifetime, and a second answer for the WebView, so it is a bigger change than this file; it is
      the right thing to build if this tier ever has to be tightened again.
    - **An external link REPLACES the document — it was never a "silent dead click".** A sandbox
      blocks *top-level* navigation; navigating the frame **itself** is never sandboxed, so a
      citation with no `target` loads that site in place of the paper (measured, every tier). Only
      `target="_blank"` is blocked, which is what `allow-popups` would enable. **PAPERS HAVE
      SHIPPED THEIR HALF** — `target="_blank" rel="noopener"` on all 5 external links, 2026-08-20
      — so this is no longer a coordination item; it is a one-sided security-tier decision on our
      side, and their half being in is harmless while we wait (a blocked link is what we have
      today). Note their correction to our suggestion: a Lua filter **cannot** do it, because
      pandoc runs `--lua-filter` before `--citeproc` and every external link in these books is
      generated *by* citeproc (measured, 0 of 5 rewritten). It is a post-process.
    - **A document's relative subresources no longer resolve at all**, which is a *tightening*:
      under `srcdoc` a paper's `../../figures/x.png` resolved against **our** origin and fetched
      from us. Moot for the papers corpus now — they inline all 9 figures as `data:` URIs and emit
      **0 relative refs** — and they note this was a precondition for the whole plan that neither
      side had listed.
    - **Tauri needs `frame-src 'self' blob:`** — `default-src 'self'` with no `frame-src` covers
      frames, and neither `blob:` nor `data:` is `'self'`. Mutation-checked in Firefox: drop the
      term and the frame is **blocked** with `securitypolicyviolation: frame-src`, while the
      `srcdoc` app/game frames and the same-origin L5 frame are untouched (3/3 render, zero
      violations with it). Verified in a real WebKitGTK WebView by baking a bundle that boots
      straight into a document page.
    - **The gate is the jump, and the demo document had to grow to carry it.** Phase 19-doc clicks
      the document's own TOC link and asserts it is *still the same document* AND that the target
      moved into view. The old demo page carried a jump-to-top link and a sentence saying
      *"internal anchors work"* — it was too short for any jump to be observable, so the **prose
      was the only evidence, and it was false**. Height is what makes it a gate; the page now
      carries a 2400px spacer before the target and 1600px after it (the tail is what lets the
      target reach the top, so the assertion can be `<100px` instead of a loose bound).
      Mutation-checked **both** ways — back to `srcdoc` fails on *"navigated the frame AWAY"*, and
      an anchor pointing at a missing id fails on *"did not move the document"*.
    - **THE GATE'S FIXTURE IS ITS BLIND SPOT, AND SAYING SO IS PART OF THE GATE.** Phase 19-doc's
      demo document is ~5 KB, so it cannot distinguish a delivery with a 2 MiB ceiling from one
      without — `data:` passed this phase every time while every real book was blank in Chrome.
      What stands in for the size property is the **delivery assertion** (`src` starts with
      `blob:`), which is a cheap spelling check doing load-bearing work *because the property is
      not observable at fixture scale*. Don't demote it, and don't read a green 19-doc as evidence
      about size. **The real check is running a real book** — `make site-serve INGEST=<a site whose
      index.html IS the 7.9 MB book>` and reading the frame: 1408 anchors, 475 `<math>`, 9 imgs,
      first TOC entry → `top: 0`, both engines. That took ten minutes and is the only thing that
      would have caught this.
    - **AND THE CHEAP ASSERTION SHADOWED THE REAL ONE AGAIN, IN THE SAME PHASE, ONE RELEASE LATER.**
      The new `src_len > 500` check inherited the old `srcdoc_len` check's *position* — above the
      behavioural one — so the first `srcdoc` mutation went red on **byte count** and never reached
      the anchor assertion. The rule two bullets up is not "order these two"; it is **every** cheap
      check goes below **every** property check, and a check that changes must be re-placed, not
      just re-spelled.
- **EVERY SHARED LINK OUT OF A PUBLISHED DOMAIN WAS UNOPENABLE — the Share button emitted the
  `self` sentinel, and `self` means *the reader's own peer*.** Reported by the operator against
  production: `https://entitycoreprotocol.org/?site=self/entity-core-protocol-main/index` →
  *"No site manifest at 'entity-core-protocol-main' (peer: 2KHcAmBU…)"*. Fixed by
  `paths::share_deep_link` (`dom/content_site.rs` now passes `output.peer`). Seven things:
  - **THE PEER-ID IN THE ERROR IS THE TELL, AND IT DIFFERS PER VISITOR.** The operator saw
    `2KHcAmBU…`; my probe of the same URL saw `2KDdokFX…`. Neither is the publisher
    (`2K4J5qsD…`, which the deployment names). A **different id per reader** is what identifies the
    sentinel as the cause rather than a publishing fault — and it is why the server-side people
    could find nothing wrong: there was nothing wrong on the server.
  - **Measured before changing anything**, fresh profile, same page, live production:
    `?site=self/…` → *No site manifest*; `?site=2K4J5qsD…/…` → **the site renders**. Two lines of
    WebDriver, and it turned a plausible story into a decided one.
  - **`self` WAS NOT WRONG WHEN IT WAS WRITTEN; ITS PRECONDITION EXPIRED.** The sentinel exists
    because a same-origin static export could not know the live peer's id at publish time, and it
    is still correct for the seeded demo site — its own doc says *"where the demo site is seeded"*.
    Under **per-domain publishing** the content lives under a durable publisher identity and is
    reached via `origins` + HTTP-poll, so the reader's own peer holds nothing. **This is the
    "a `cfg` line is not the code path" family: the constant was right, the world moved.**
  - **THE UNBLOCKING CONDITION WAS WRITTEN DOWN IN THE FUNCTION'S OWN DOC AND NOBODY SPENT IT.**
    `render_share_button` said a real permalink needed *"the hosting-identity piece: the live peer
    publishing its OWN tree, or a registry telling the app where this peer's site is published."*
    **Both shipped** — durable per-domain identities and a registry signing `name → peer-id` — and
    the note sat there through both. Same shape as the TURN limitation and the `--strict-links`
    default: **state a concession's expiry where you make it, then go back and spend it.**
  - **THE STATIC EXPORTER ALREADY HAD IT RIGHT** (`static_export.rs`, `?site={peer}/{site}/{page}`,
    with a test pinning the exact href). Two renderers of one surface, one of which took the
    lesson — the *third* instance of that shape in this file after the nav flattening and the
    responsive/overflow handling. **When you fix a content-site presentation bug, check the other
    renderer.**
  - **THE FIX HAD TO BE MADE OBSERVABLE BEFORE IT COULD BE GATED.** The link was captured in the
    click closure and written to the clipboard — unreadable in headless, permission-gated, so *no
    browser gate could ever have seen this*. `data-share-link` on the button is the same move as
    `data-app-state-seq`. **A property that only exists inside a closure is a property no gate can
    hold.**
  - **Two gates, both mutation-checked in both directions, and they prove different things.**
    `a_shared_link_carries_the_publishers_peer_not_the_readers` (native) proves the **rule**;
    **Phase 21** proves the renderer **calls** it — and Phase 21 needed no new rig, because a
    foreign site is already on screen there. Reverting the renderer alone leaves the native test
    green, which is exactly why both exist. The `assert_ne!`/negative half is the load-bearing one:
    `self` is a *well-formed* link that opens nothing, so every positive shape assertion passes
    while the bug is live.
- **A CROSS-DOMAIN `site:` LINK IS NOT EXPRESSIBLE — the exporter is missing an INPUT, not a check
  — and the contract forbidding one had no enforcement point for a year.** `static_href` resolves
  `site:other/page` against the **current** peer unconditionally, so under per-domain publishing
  (`cgid-10-235`) a target on another domain ships as a 404 under *this* domain's peer. Six things:
  - **`CrossPeer` IS AFFECTED IDENTICALLY, which the production finding did not say.**
    `projection_href` emits a **host-less, root-absolute** path, so `entity://{other-peer}/sites/x`
    also lands on the current domain. So "consult the export set" is not the fix and neither is
    "honour the explicit peer" — there is **nowhere to put a hostname**. Any proposal that does not
    introduce a `peer_id → origin` input is not a fix, it is a differently-wrong href.
  - **MEASURE BEFORE BUILDING: the whole live corpus carries SEVEN of these**, three files, one
    domain (foundation), all outbound — billslab ships its sites together, protocol and registry
    emit none. A day of exporter + deployment-config plumbing was scoped and **declined** on that
    number. The measurement is a ~20-line script over `content/**.md` classifying source vs target
    domain by the `{domain}-{site}` id convention; run it again rather than trusting this count.
  - **IT WAS NEVER MISSING INFRASTRUCTURE — `entity-core-papers/docs/CROSS-SITE-LINKS.md` §Type 2
    already requires a cross-domain link to be an absolute `https://` URL**, reconciled with arch.
    Seven hand-authored links violate it because **nothing checked**, and the render tool's
    chokepoint only *generates* `site:` for siblings — a hand-written one passes straight through.
    Before designing a mechanism for a cross-repo defect, **go read whether the two sides already
    agreed a rule and nobody enforced it.**
  - **The guard is `LinkAudit` → `ExportReport::dangling`, and it audits NAV as well as bodies** —
    the portal-index generator emits cross-site nav links, so a domain router is exactly where an
    out-of-set target appears and a body-only check would miss it. `export_bare_root` audits
    **nothing**: one site has no set to be outside of, and flagging its deliberately-namespaced
    outbound hrefs is the cry-wolf failure that gets a guard routed around with `|| true`.
  - **WARN BY DEFAULT WAS A DATED CONCESSION, AND THE DATE CAME — IT REFUSES BY DEFAULT NOW
    (2026-08-23).** The entry that stood here said *"flip the default once those seven land"*.
    They landed: `entity-core-papers` `ef3f662` swept them, and it was **verified rather than
    taken on report** — 0 cross-domain `site:` refs across all five domains of the corpus, and
    all four published domains emitting clean under the refusal (412 pages, 0 dangling). The
    escape hatch is `--allow-out-of-set-links` / `ALLOW_OUT_OF_SET_LINKS=1`, for a deliberately
    partial set mid-migration; **`--strict-links` is accepted and is now a no-op**, because other
    repos' pipelines still pass it and removing it would fail their builds on an unknown flag
    while they ask for exactly the default. Three things worth carrying:
    - **The gate is on the DEFAULT, not on the refusal** — `refuses_out_of_set_links` is a free
      function with a test asserting that a *bare* argv refuses. The refusal itself was already
      tested; what had no gate was the thing that changed, and what stood between the flip and a
      silent slide back to warning was one `!` in the middle of a several-hundred-line
      `run_publish`. **When you spend a concession, gate the default, not the behaviour.**
    - **Verified in BOTH directions against the real corpus, not a fixture** — the live tree
      publishes clean with no flags; an injected out-of-set target produces 7 errors and exit 1;
      the hatch still emits; `STRICT_LINKS=1` still refuses. The fixture-is-the-blind-spot rule
      applies to a *policy* flip as much as to a size ceiling.
    - **`--verify` is unaffected, and that was checked rather than assumed.** The `verify_only`
      return still sits above every emit-time guard (publish.rs:223 vs the guard at ~1560) — the
      structural fix made after an emit-time guard once failed a read-only pass. Making a guard
      *stricter* raises the cost of that ordering being wrong, so re-check it when you do.
    The standing rule is unchanged and now has a worked example on both ends: **state a
    concession's expiry where you make it, or it becomes the design — and then go spend it.**
  - **A `site:` TARGET IS A NAME, so the long-term answer is registry resolution — NOT a by-peer-id
    reverse index**, which is the obvious framing and the wrong level. A binding is
    `name → peer-id + transports(origin)`, i.e. *exactly* the tuple a seed table would hard-code —
    which is what makes a seed table a **pre-resolved registry lookup** rather than a parallel
    mechanism. Design + the open questions for arch: `docs/plans/DESIGN-CROSS-DOMAIN-SITE-LINKS.md`
    (backlog B-4). Note the trust split: a **static absolute href** does not trip B-3 (it is an
    ordinary web link and the destination's own SPA takes over); a **registry-resolved origin
    does**, and wiring that without threading `SignedSession` is D2's violation as a convenience.
- **THE PAPERS' CDN DEPENDENCY AND BROKEN FIGURES ARE BOTH CLOSED — PAPERS FIXED THEM, and the
  entry that stood here saying otherwise is a MEASUREMENT WITH A SHELF LIFE.** As of their
  2026-08-20 build (`HANDOFF-2026-08-20-BROWSER-RUST-HTML-BOOKS-SANDBOX.md`): `<script>` tags
  **2 → 0**, network references at read time **1 → 0**, math **raw TeX → 475 native MathML
  elements**, figures **9 relative `<img>` → 9 `data:` URIs, 0 relative**, external links **5 bare
  → 5 `target="_blank" rel="noopener"`**. Reproduced here against
  `book/output/entity-system-book.html` — all six numbers exact. Two things worth carrying:
  - **The broken figures were worse than we filed them.** We reported them as broken *as published
    on GitHub*; they were broken **in every book, from every serving location**, because pandoc
    reads their chapter bodies on stdin and copies the `src` string through unresolved. The PDFs
    were fine (Typst embeds), which is why it survived. It is now a **build failure**, which is the
    actual fix — the inlining is just the mechanism.
  - **The file got 5.6× bigger — 1.4 MB → 7.9 MB — and that is the number that matters to us.**
    The figures are the increase. Budget the full-corpus book at ~8 MB per open, not 1.4 MB; it is
    what put us over Chrome's `data:` cap and it is what the mount's revoke accounting is sized
    against. Their `--embed-resources` was delivered by a different mechanism than we asked for
    (their chapters are hand-assembled fragments, so the flag needs `--standalone`); the *effect*
    is what we wanted and the flag name in our old ask was wrong.
  - **BOOK ANCHORS ARE NAMESPACED BY PAPER NUMBER NOW — `#introduction` is `#p05-introduction`,
    and if you ever write a fixture or a deep link that is the form.** Their concatenated bundles
    duplicated ids (every one of 15 papers contributes an `introduction`, plus `fn1`/`fnref1` per
    chapter), because each chapter is its own pandoc pass and pandoc dedupes *within* a document.
    A browser resolves to the first match, so all 16 landed on Paper 00 — or looked dead when you
    were already there. **This is what our "some links work, some don't" actually was**, and it is
    a different defect from our blob-revoke bug that presented identically. Fixed at assembly, with
    `#partN` and `#ref-<key>` deliberately held out (they are cross-fragment targets emitted by a
    raw template, which is why `--id-prefix` was the wrong tool — it rewrites the links and not
    those targets). Verified here across **all 22 bundles: 6592 fragment links, 0 ambiguous, 0
    dangling, 0 duplicated ids**, and driven through the shipped app — 15 `Introduction` entries in
    full-corpus land at **15 distinct positions**, every one at `top≈0`.
    - **The tell was inside their own clean bill of health, and it is worth stealing: "793 unique
      targets, 836 ids, 0 dangling" was reported as *the navigation graph is sound* while the
      document carried 949 id attributes.** Dangling and duplicate are **two properties**; they
      measured one and stated a conclusion needing both, and it sent us hunting on our side. When a
      count of *unique* things sits next to a count of *things*, the gap is the finding.
  **Standing lesson: an entry recording another repo's defect is a claim about a moment.** This one
  said "neither is ours to fix" and was correct; it went stale the same week, and a seat acting on
  it would have widened a sandbox or vendored MathJax to solve a problem that no longer exists.
- **THE STATIC EXPORT FLATTENED EVERY NESTED NAV INTO ONE ROW, AND THE MARKUP WAS NEVER THE
  PROBLEM — `page_css`, `src/content_site/static_export.rs`.** Reported from the outside as *"the
  menu items just shoot off like every item across the top — it's a mess"*. `render_nav_items`
  has always emitted a correct tree (a child `<ul>` nested inside its parent's `<li>`); the
  stylesheet had `.site-nav ul` **and** `.site-nav li` both `display:flex` **unscoped by depth**,
  so a nested `<ul>` became a flex item laid out *beside* its parent label and every descendant
  collapsed onto one line. Six things:
  - **Measured in a real browser, both ways, with only the stylesheet varying.** Pre-fix, a
    4-item / 2-group nav rendered **8 links on one 27px row** — and not even aligned (leaves at
    y=17, parents at y=20, from `align-items:baseline` on mixed-content `<li>`s). Post-fix: three
    rows, children x-aligned *under* their parent at 13px muted vs the parent's 15px. The fix is
    `.site-nav>ul` for the top-level row plus a `.site-nav ul ul` column rule.
  - **The bundled demo nav is FLAT, so `make site` alone cannot reproduce it.** Five items, no
    children — the fixture is the blind spot again. Reproducing needs a `site.manifest.json` whose
    `nav` carries `children` (the real corpus does: billslab's *Papers* group header is what
    `parse_nav`'s own comment cites). Build one under `/tmp` and `make site INGEST=… OUT=…`.
  - **THE LIVE APP ALREADY SOLVED THIS AND THE STATIC EXPORT NEVER LEARNED IT.** `dom/content_site.rs`
    caps the bar at `NAV_INLINE_MAX = 4` with a *More ▾* overflow **and** a ≤768px hamburger, and
    its doc comment records fixing an off-screen mobile panel. The static export has **no cap, no
    overflow, no responsive handling** — two renderers of one surface, one of which took the
    lesson. That asymmetry is the thing to check when a content-site presentation bug arrives.
  - **Groups are `<details>` dropdowns now, and the panel is anchored to the HEADER, not to its
    own `<li>` — that is what makes off-screen impossible rather than unlikely.** The first
    version positioned the panel against its `<li>`, so a group near the right edge opened
    partly off the viewport with no way to reach it. `left:0;right:0;top:100%` against a
    `position:relative` `.site-header` spans the header instead, so it **cannot** overflow
    horizontally at any width. Long groups get `column-width:190px` + `max-height:60vh`; the real
    23-link group lays out in **6 columns at 1280px, 5 at 800px**, nav **30px closed**.
  - **CLOSED BY DEFAULT, and "you are here" is a HIGHLIGHT — do not "improve" this to `open`.**
    A static exporter knows the current page at render time and the live app does not, so shipping
    the current group `open` is the obvious move. It is wrong: the panel floats (so it does not
    shove the article down), which means an auto-opened group **covers the page on arrival**, and
    a `<details>` cannot close on an outside click without JS — the reader would dismiss a panel by
    hand on every page. The render-time knowledge marks the group instead. Both halves gated.
  - **Clicking the LABEL navigates; clicking anywhere else in the `<summary>` toggles.** Measured,
    not assumed — an `<a>` inside a `<summary>` navigates cleanly in Firefox while the rest of the
    summary (including the `::after` chevron) still toggles. That is what lets one control do both
    without JS. If that ever stops holding, the fallback is a non-link group label.
  - **THE MARKER IS DRAWN WITH BORDERS, NEVER A GLYPH — `font-size` DOES NOT SIZE A GLYPH, and the
    first fix for "the arrows are tiny" changed nothing because of it.** That round hid the native
    marker and drew its own with `content:"\25BE"` at `font-size:13px`; the operator reported back
    that it was *still a tiny little triangle*, and they were right — U+25BE is BLACK
    DOWN-POINTING **SMALL** TRIANGLE, a subscript-sized mark occupying a fraction of its em box, so
    raising `font-size` grows the box the glyph sits in and barely moves the ink. Measured on one
    page in one font: the glyph's ink is **7×7 in a 6.5×13 advance box**; the replacement is a 9×9
    element with two 2px borders rotated 45° — **12.7px of diagonal ink**, sized in the units the
    complaint was about, identical in every font, and it rotates to `-135deg` when open instead of
    flipping a glyph. **If a control's size is the property under review, do not express it as a
    character.** Gate: `the_disclosure_marker_is_drawn_rather_than_typed` — empty `content` plus
    explicit `width`/`height`, and it fails on `font-size` **by name**, because that is the
    regression that already shipped once. Mutation-checked.
  - **AN AFFORDANCE THAT IS BIGGER THAN IT LOOKS IS A DEFECT EVEN WHEN EVERY CLICK LANDS.** The
    second half of the same report — *"it's hard to see what the hitbox is"* — was the more
    accurate one, and it is a different bug from the size of the mark. The whole `<summary>` has
    always toggled (everything but the `<a>`), but nothing said so: no padding, no hover feedback,
    so the only thing that looked pressable was a 10px mark. It is a pill now — hover and `[open]`
    both raise a background and a border — so the target advertises its own size: **121×36
    measured, against ~10px of apparent affordance.** A `<summary>` is focusable, so
    `:focus-visible` paints an outline too. Note what was **not** the defect: the resting marker
    is 4.7:1 against the nav background and the non-text floor is 3.0:1, so **contrast was fine and
    fixing it would have fixed nothing** — measure the three candidate properties (size, contrast,
    affordance) before picking one.
  - **A `rm -rf` OF A BIND-MOUNTED PUBLISH DIR LEAVES THE CONTAINER SERVING A DELETED INODE.** The
    republish-then-reprobe loop is `rm -rf dist-x && make site OUT=dist-x`, and a `cors-serve`
    container bind-mounting `dist-x` keeps the *old* directory alive by its inode: it serves the
    previous build forever, and the browser probe then fails somewhere unrelated (here, a 500 out
    of `execute/sync`) rather than saying the tree is stale. Recreate the container after any
    `rm -rf` of its mount, or publish into a fresh directory name.
  - **USE THE REAL CORPUS — it is staged on disk and the invented fixture hid two of the three
    defects.** The publish-staging directory in the coordination tree holds the actual authored
    sources per cut (7 sites, 412 pages at the cut this was measured on); ask the operator for the
    path if you do not have it. `make site INGEST=<that dir> OUT=dist-real` publishes the lot. Measured shape, which no invented nav matched: **max depth 1 everywhere**,
    and one group of **23** (`billslab-entity-system`'s *The Papers*) against a demo whose groups
    were 3–4. The tower and the off-screen panel are only visible at real scale. Third instance of
    *the fixture is the blind spot* in this file, and the first where the real data was sitting
    right there.
  - **`grep -c live-banner` COUNTS THE INLINED CSS, NOT THE BANNER.** The stylesheet carries
    `.live-banner{…}` whether or not the markup is emitted, so **1 = CSS only, 2 = CSS + banner**.
    And the two publish routes disagree by design: `site-serve` passes `--live=` unconditionally
    so it always banners, while plain `make site` emits none unless `LIVE=` is set. Overwriting a
    served tree with the other route's output makes the banner appear and disappear, which reads
    as a flaky product and is not one.
  - **`static_export` is `#[cfg(not(target_arch = "wasm32"))]` — this CSS never ships in the
    browser wasm.** `make wasm` is unaffected by any change to it; the artifact that matters is
    the **native** binary `make site` runs. Verified by grepping the rebuilt wasm and finding
    *neither* the old nor the new rule, which is the honest way to learn that.
  - **A HEADLESS FIREFOX WINDOW WILL NOT GO BELOW ~500px, AND IT LIES SILENTLY.** Requesting a
    360px window through WebDriver `window/rect` yields `innerWidth: 500`, and the measurements
    come back identical to the 1280px run — which reads as "the layout is broken and ignoring the
    viewport". **Assert `window.innerWidth` in any responsive probe** before believing a narrow-
    width result; a conclusion about mobile overflow was drawn and withdrawn on exactly this.
- **THE APEX IS A WASM SPA, SO ANYTHING THAT DOES NOT RUN JS SAW AN EMPTY BODY — `index.html`'s
  `<noscript>` landing, gated by `make noscript-check`.** A search crawler that does not execute
  JS, a text browser, a reader with JS off and a link-preview fetcher all got: no text, no links,
  no description, and a boot spinner frozen on *"Loading Entity Browser…"* forever — while the
  published sites sat under `/sites/` as **plain crawlable HTML that nothing linked to**
  (`static_export` has always written a `{out}/sites/index.html`). Five things:
  - **NO GATE IN THIS REPO COULD EVER HAVE SEEN IT.** The e2e suite drives a browser with
    **JS on, by definition** — there is no phase, and can be no phase, that observes this
    surface. That is why `noscript-check` is a separate tool with its own make target rather
    than a 54th phase. **When a surface is defined by a capability being ABSENT, the suite that
    assumes the capability is structurally blind to it** — the same shape as every WebRTC gate
    being browser↔browser while the desktop had no WebRTC.
  - **The control is the whole gate.** "The landing is visible with JS off" is satisfied by a
    landing that is *always* visible — which would mean every ordinary visitor sees the fallback
    under the app. So it drives **both** postures and requires the JS-on run to hide it.
    Mutation-checked both ways: drop the `!important` → red on visibility; drop the inline
    `display:none` → red on the control.
  - **Rendering and crawlability are DIFFERENT PROPERTIES and both are asserted.** A crawler
    parses bytes and never runs our CSS, so `href="sites/"` must be in the served **source** —
    a link reachable only through a stylesheet is not reachable by it at all. Asserting only
    "the element is displayed" would pass a page no crawler can follow.
  - **It is an anchor, NOT a `<meta http-equiv="refresh">`, deliberately.** A refresh inside
    `<noscript>` would bounce a no-JS human straight to `/sites/`, but to a crawler it turns the
    apex into a *redirect*: the apex stops being an indexable page and its signal moves to the
    sites index. An anchor keeps the apex indexable AND hands the crawler the path in. One line
    to reverse if that trade ever changes.
  - **The href is RELATIVE (`sites/`)**, matching `static_export::write_landing_redirect`'s own
    stated reason — it works at an apex and under a `PREFIX` sub-path alike, which is exactly
    `make site-dist`'s invariant that the SPA and the static tree share a root.
  - **Observed and NOT acted on: a no-JS agent still downloads the wasm.** trunk emits
    `<link rel="preload" … as="fetch" type="application/wasm">`, and a preload is a resource
    hint the browser honors whether or not scripts run — measured, a JS-disabled Firefox fetched
    the whole `_bg.wasm`. So crawl budget is spent on a bundle the crawler cannot use. The
    **release** size (opt-level=z + LTO + `wasm-opt -Oz`) is the number that matters, and it is
    **MEASURED NOW — 7,609,107 bytes** (`make dist-web`, 2026-08-24), against the 29,669,867 of the
    debug build this note was written beside: **3.9× smaller, and the debug figure overstated the
    trade by that factor.** Removing the preload would cost every real visitor cold-start time, so
    this is a trade for whoever owns the release, not a bug — but price it at 7.6 MB.

## Apps & embedded programs

- **ONE Apps window over BOTH app-sets — `Games` is a retired type key, and a selection is now
  `(set, id)`.** The `games`/`apps` split is a **storage** partition (`/{peer}/apps/{set}/…`,
  decided at ingest from a publisher's `type`) and never stopped being one; it merely stopped
  being a *window* partition. Five things earned on the merge:
  - **A bare id is ambiguous now, and `app_save_path` already told us why** — it is set-keyed
    precisely because ids collide across sets. The DOM event carries `"{set}/{id}"`
    (`parse_selection`); `resolve_selected` still accepts the bare form the pre-merge windows
    persisted, resolving its set by lookup, or a returning user is bounced to the grid for a
    reason they cannot see. **Any new surface naming an app owes the pair.**
  - **A retired window type needs a `canonical_window_type` alias, not just a deletion.** A
    persisted workspace, a baked `ENTITY_STARTUP_WINDOW_TYPE`, or a shell verb naming `Games`
    resolves to no factory and **silently opens nothing** — the same shape as `System Backend`,
    and pinned by `the_retired_games_key_still_opens_the_launcher`.
  - **The chip row is derived from the data, and a chip that would show an empty grid is not
    rendered** (`apps::category`); one surviving chip renders **no row at all**, because two
    buttons showing the same grid is chrome claiming to be a control. Counts ride in the label so
    a chip cannot be mistaken for a broken filter. The fold is a real table — the published corpus
    carries **ten** fine categories (cards/strategy/puzzle/arcade/word · audio/art/utility/music/
    productivity, measured across 33 apps), not the four coarse names on screen — and its test
    asserts against that measured left column, because a mapping table is only worth something if
    what it maps FROM is real. **The set decides a game, not its fine label**: a rhythm game
    published `category:"music"` is still a game.
  - **The merged window watches BOTH sets' prefixes, and the save prefix too.** On the Worker arm
    a read lands in the cache mirror only for a subscribed prefix, so a set this window reads but
    does not watch would be silently absent from the grid. The **save** prefix was the latent one:
    the player has always read `app_save_path` at render time *without* watching it, working only
    because the write that put it there seeded the mirror in the same session — a post-reload read
    on the Worker arm would have come back empty. The Saves panel made that load-bearing (it lists
    a prefix having written nothing), so both are watched at the factory now.
  - **Entity Native Apps is a WINDOW under Apps & Content, not a group** (identity key still
    `Programs`, label already `Entity Native Apps`). It had its own palette heading for exactly one
    session and the operator cut it on sight: **one row under its own banner reads as a whole area
    of the product**, when what it is is a second launcher. The argument for promoting it — that a
    published HTML bundle and an L5 compute program are different things to run — is true and does
    not reach as far as a heading; it is answered by sitting directly *after* Apps. Keep the menu's
    three groups. Generally: a group is a place with several things in it, and a category invented
    for one window is a claim about the product, not about the window.
- **AN APP CAN KEEP THE SCREEN AWAKE — `allow="screen-wake-lock *"` on the app iframe, and the
  TRAILING `*` IS THE WHOLE FIX.** Reported from the outside as *"I watch Warlord run the AI and
  the screen keeps blanking"*. `screen-wake-lock` is **Permissions Policy, not sandbox** — denied
  in a frame by default, and only the embedder can delegate it, so it is the one thing an app
  cannot grant itself. Five things:
  - **THE SPELLING WE WERE ASKED FOR IS INERT FOR OUR APP TIER, AND IT FAILS SILENTLY.** entity-apps'
    `EMBEDDING.md` §5 says `allow="screen-wake-lock"`. That defaults its allowlist to `'src'` — the
    origin of the frame's `src` — and a sandboxed **`srcdoc`** frame has an *opaque* origin and no
    `src` at all, so the allowlist matches nothing. Measured, Firefox 149, our two tiers:

    | sandbox | allow | granted |
    |---|---|---|
    | `allow-scripts` | *(none)* | no |
    | `allow-scripts` | `screen-wake-lock` | **NO** ← the trap |
    | `allow-scripts` | `screen-wake-lock *` | yes |
    | `allow-scripts allow-same-origin` | *(none)* | yes |
    | `allow-scripts allow-same-origin` | `screen-wake-lock` | yes |

    So **`*` is not "grant it to everybody" here — it is the only spelling that names an opaque
    origin at all**, because there is no token for one. Note the L5 row: `allow-same-origin` gets
    it with no attribute (the default allowlist for this feature is `self`), which is exactly why
    a host that only tests its trusted tier sees nothing wrong.
  - **GRANTED TO EVERY APP, DELIBERATELY.** The manifest has no machine-readable way to declare the
    need (entity-apps says so), the failure is silent, and the platform already bounds the grant —
    a screen wake lock is **released automatically when the document becomes hidden**, so a
    backgrounded app cannot hold your screen on. That auto-release is also why an app must re-take
    the lock on `visibilitychange`; that half is the app's and their SDK does it. A "declare it
    first" scheme would buy nothing here and cost a silent failure per undeclared app.
  - **DEAD OFF A SECURE ORIGIN, AT EVERY TIER.** Measured on a plain-http LAN address:
    `navigator.wakeLock` is **undefined**, so the attribute is inert no matter how it is spelled.
    That is the `pair-serve` / desktop-app-server two-machine path — a phone reaching Tori at
    `http://192.168.x.x:8081` gets no wake lock and nothing we can do fixes it but https. Add it to
    the standing list of what an insecure origin costs (beside `getUserMedia`, so the QR scanner
    and this die together).
  - **THE GATE ASSERTS THE GRANT FIRST AND THE SPELLING LAST** —
    `a_running_app_can_hold_a_screen_wake_lock` runs `wakeLock.request('screen')` **inside** the
    real frame (`enter_frame`; the host page holds the feature regardless, so asking from outside
    proves nothing about what we delegated). The tempting gate is `getAttribute('allow')`, which
    **passes for the inert spelling** — a spelling-only gate is green over a completely broken
    feature. Third instance of *the cheap check must come second*, and the first where the cheap
    check would have been green on the exact bug. Mutation-checked both ways (bare token → red on
    `NotAllowedError`; attribute removed → same).
  - **ENTITY-APPS' OWN LAUNCHER HAS THIS BUG, MEASURED IN IT.** Their `templates/index.html:101`
    ships `sandbox="allow-scripts allow-downloads" allow="screen-wake-lock"` with the bundle
    delivered as `frame.srcdoc` — the denied configuration exactly. Driven against their served
    `dist/` over localhost with their real Warlord bundle, only the attribute varying:
    bare → `denied · NotAllowedError`, `*` → `granted · screen`. So a host-side delegation test
    that reads the **attribute** cannot see this class, which is the shape they report having.
    Routed back with the reproduction:
    `docs/status/ROUTING-2026-08-23-b-wake-lock-the-bare-token-is-inert-in-a-srcdoc-frame.md`.
  - **`git worktree list` APPLIES TO SIBLING REPOS, AND THIS IS THE FIRST TIME IT BIT THERE.**
    `entity-apps` on **`master`** has **zero** occurrences of `wakeLock` — SDK, launcher, docs,
    every app. The entire feature lives on the **`warlord`** branch in a separate worktree
    (`entity-apps-warlord`, 39 hits). Reading `master` and reporting "they have not built it"
    would have been a confidently wrong cross-repo finding, off a partial search that *looked*
    exhaustive. **`git log --all -S<symbol>` across every sibling checkout before any claim about
    another repo's state** — it is what found the branch, and it costs one command.
- **AN APP CAN FILL THE SCREEN NOW — `⛶ Full screen` in the player bar, and the point is the two
  bars it removes.** The Apps window already had two ways to get bigger and they read as one:
  `▢` on the window header (the *window* covers the viewport) and `⤢ Expand` in the player bar
  (the *stage* fills the window). Both together still leave the window header, the player bar and
  the browser's own chrome above a game. Rules in `views::games::stage` (native-tested), the DOM in
  `dom::games::render_player`, the API wrappers in `dom::util`. Seven things:
  - **`web_sys`'s `Element::request_fullscreen` DROPS THE PROMISE — never call it.** It is typed
    `-> Result<(), JsValue>` in 0.3, so it invokes the method and discards the `Promise` the method
    actually returns; that Promise **rejects** on every refusal (no transient activation, an engine
    with fullscreen off), and a dropped rejecting Promise is this file's standing footgun. Same for
    `Document::exit_fullscreen`, typed `-> ()`. Both go through `Reflect` + `call0` in
    `util::call_consuming_promise` so the return value can be consumed. **Grep for other web-sys
    methods typed `-> Result<(), JsValue>` whose IDL returns a Promise before assuming the binding
    is safe.**
  - **`document.fullscreenElement` RETARGETS TO THE SHADOW HOST, so it is useless to us.** Every
    window renders inside a shadow root; the comparison you reach for is false for every element
    you would ask about. `util::is_fullscreen` asks `el.matches(":fullscreen")` instead, which is
    unaffected. Measured working on a shadow-tree element.
  - **THE CHROME IS RECONCILED FROM `fullscreenchange`, NEVER FROM OUR OWN CLICK** — a request can
    be refused, and Esc exits without any click at all. The event fires **at the element** (not
    only at the document), so the listener sits on the player. Mutation-checked: remove it and the
    gate goes red with the player at 1366px and the bar still saying *"Full screen"*.
  - **THE `:fullscreen` AUTHOR RULE IS DEAD CODE, MEASURED — and the first version shipped it with
    a confident comment.** The UA stylesheet already gives a fullscreen element
    `position:fixed` + `width/height:100% **!important**` at the full viewport, and the wrapper's
    own inline background survives, so nothing an author writes can matter. It is *deleted*, with
    the measurement in the CSS doc comment so nobody re-adds it "for safety". **The way this was
    caught is the transferable part: a mutation that does not go red is a finding about the code,
    not about the gate.** Joining the two spellings into a selector list stayed green — and the
    reason was not the one guessed (`CSS.supports('selector(:-webkit-full-screen)')` really is
    **false** in Firefox and a list of the two really does parse to **zero** rules, both measured);
    it was that the assertion could not see the rule's absence either way.
  - **A TRUSTED CLICK IS REQUIRED AND THE SUITE'S USUAL `el.click()` CANNOT PROVIDE ONE.** Every
    other control in this suite is pressed by script, which produces an event with **no user
    activation** — fine everywhere else, fatal here: measured, a script press is refused with
    `TypeError: Fullscreen request denied` while a WebDriver pointer sequence is granted. So
    `trusted_click_at` locates the element by script (for its rect) and presses it with the driver.
    **Any future gate for a gesture-gated API — clipboard write, media capture, popups — needs the
    same treatment, and the failure looks like the feature being broken.**
  - **POLL FOR THE CHROME, NOT FOR `:fullscreen` — the engine flips the pseudo-class and dispatches
    the event as TWO STEPS.** The first version of the gate polled on `fullscreen` alone, sampled
    the instant between them, and reported a state the app was never in; it read exactly like the
    product bug it was written for. The predicate now names the reconcile. **Generally: when a gate
    waits on a browser state change that our code reacts to, wait on OUR half.**
  - **The `<iframe>` deliberately carries no `allow="fullscreen"`**, and it never needs one: the
    host takes the *player* to the screen and the frame rides along as a descendant. The permission
    governs the app calling `requestFullscreen` **itself** — i.e. any published bundle covering the
    screen at a moment of its own choosing with our chrome gone. Adding the token is a trust-tier
    change, not plumbing.
  - **UNVERIFIED: the Tauri WebView.** The gate is Firefox. WebKitGTK is a different engine and this
    file already records it surprising us badly once (no `RTCPeerConnection` at all), so do not
    claim the desktop works. It **degrades honestly rather than silently** — the button is rendered
    only when `document.fullscreenEnabled` is true — but "the button is absent on the desktop" and
    "it works" are different answers and nobody has looked.
- **INTERACTIVE LIFE IS IN — `app/life-edit`, a FOURTH embedded program, and the first one that
  exercises the standard controller end to end.** Re-pulled from workbench-go via `make
  program-fixtures` (`tools/program-dump/main.go` gained a `life-edit` entry). It is a **separate
  program from `life`, not a mode of it** — its own root, its own state shape
  (cursor/gen/pkeys/paused), its own step — so both ship. Five things:
  - **`life`, `snake` and `asteroids` never proved the role parser.** The first two bind no
    key-set at all and asteroids predates control roles, so `controls.rs` had unit tests over
    hand-written keymaps and **no authored program that used it**. life-edit declares four axis
    bits and three actions (toggle/regen/pause) with glyphs and behaviours, which is what turns
    that parser from tested into exercised.
  - **The cross-impl oracle passed first try** (`interactive_life_matches_go_oracle_across_every_control`)
    — 16 ticks, our evaluator against go's, over a schedule that drives every control. **Every
    press in that schedule is followed by an explicit RELEASE**, and that is load-bearing: a
    scheduled input persists until the next one and the program edge-detects against `pkeys`, so
    "press pause at t8, press again at t11" with no clear between is ONE press held for three
    ticks and the resume never happens.
  - **WORKBENCH-GO'S D-PAD WAS INERT FOR THREE WEEKS WITH GREEN SUITES, AND THE LESSON TRANSFERS
    DIRECTLY** (their `7729cb5`). Two defects either side of their language boundary; the one that
    is OURS to care about is in the **generic host**: a host that samples its input ports only at
    tick time never observes a press that begins and ends between two ticks. At 6 Hz that window
    is 167 ms and a click is ~25 ms — they measured **1 of 6 d-pad presses landing**. Their fix is
    a host-side input QUEUE ("every value offered to a port is observed by exactly one tick, in
    order"). **Ours is different and it is in the DRIVER** — `onscreen::MomentaryGuard` holds the
    release for one tick period (`min_hold_ms`). Two implementations of one generic host with two
    different answers to one race; ours is verified working (below), theirs is strictly stronger
    for rapid repeated taps. **Do not "align" them without reading both** — and if the host ever
    grows a queue, `min_hold_ms` becomes redundant rather than wrong.
  - **THE GATE ASSERTS PROGRAM STATE, NOT A HOST STAMP — e2e Phase 2h.4.** This is the distinction
    go's session paid for: *"That verification was honest and it was about rendering. The buttons
    rendered perfectly."* Phase 2h.2d, our existing on-screen-pad check, is on the WRONG side of
    it — it asserts `data-app-host-input == "dir:1"`, i.e. the host's own stamp saying it wrote to
    the target, which **a program that never observes the write also satisfies**. 2h.4 presses
    `press:pause` with a real pointer down/up and waits for the program's **status caption** to
    flip to its paused glyph — projected by the program's own `status` port from the `paused`
    field its step wrote, so no amount of correct plumbing can fake it. Measured:
    `POP 0085 ▶` → `POP 0071 ⏸`. **Mutation-checked against go's exact bug**: set `min_hold_ms`
    to 0 and it goes red with the button present and the press dispatched
    (`has_pause_btn: true, paused: false`).
  - **A grid probe that only matches a substring cannot count two tiles.** Phase 2h.3 asked
    `names.some(n => n.includes('Life'))` — true of "Interactive Life" too, so one tile satisfied
    both checks and a missing program would have ridden green. It now excludes the interactive
    name and asserts `life_tiles == 2` structurally, and the phase's completion line **reports the
    measured tile count** instead of the literal "3 built-in programs" it had gone stale carrying.

## File transfer & chat

- **File transfer: the TRANSFER is transport-agnostic; what the browser lacks is a RECEIVING
  side.** Upload is picker → `File::array_buffer()` → bytes → `Action::UploadFile` →
  `ops::execute` op **`write`**, with a read-back **`read`** verifying it landed
  (`src/dom/file_transfer.rs`, `app.rs::handle_upload_file`); browse is `list`, download is
  `read`. All of it goes through `Peers::execute` over **whatever pooled connection reaches the
  target** — a WebRTC data channel as readily as a WebSocket. **Nothing here is WebSocket-specific.**
  What *is* native-only is the serving handler in use today: `entity-local-files` is
  `#![cfg(not(target_arch = "wasm32"))]` and the share is mounted by the Tauri backend
  (`ensure_share_root` → `local_files_handler().add_root`). So the browser is **client-only** —
  it can push and pull *through that handler*.
  **Do not repeat this seat's 2026-08-16 error:** it read `entity-local-files`' `cfg` line, did
  **not** read the nine-line upload path, and concluded to arch that file transfer was
  *structurally incapable* of riding WebRTC in any topology. Withdrawn same day. A `cfg` on one
  handler says nothing about the transport the bytes take.
  - **A browser CAN serve a file now — `src/file_offer.rs`, and the answer was a different
    handler, not new plumbing.** `system/content` (`ingest` §6.3 / `get` §6.2) is registered on
    **every** peer, wasm included (the `content` feature is on for both `entity-peer` and the
    wasm SDK). Offer = chunk locally (§3.2 fixed, 256 KiB) → `ingest` blob+chunks into our own
    namespace → publish a manifest at `app/entity-browser/offers/{blob-hex}` (name/size/blob/from
    — content is hash-addressed, the manifest is what gives a hash a *filename*, and keying by the
    blob hash makes a re-offer an idempotent overwrite). Pull = list the sender's offers over
    `system/tree`, then **walk the closure**: fetch the blob, decode the chunk list it names,
    fetch the chunks in `GET_BATCH_SIZE` batches, `reassemble`. Upstream's `ensure_closure` is
    not usable here — it takes a `&dyn Dispatcher`, which only the Direct arm can produce.
    **Pull, not push** (a read grant, not a stranger writing into your tree). The shell verbs
    `offer` / `offers` / `pull` were the first surface; the **window is the shipped one** now —
    the File Transfer picker raises `Action::OfferFile` (untargeted: an offer is published on
    *our* side, so it renders with no peer selected and outside the authorization gate — gating it
    would hide the only send a browser↔browser pair has behind a question it never asks). Gates:
    `make e2e-webrtc-file` (two browsers, 700 KB / 3 chunks via the Shell **and** a second file
    offered from the window's picker, 18 checks) +
    `a_file_offered_by_one_peer_is_pulled_byte_for_byte_by_another` (native, mutation-checked).
    Use a **multi-chunk** payload in any new transfer test — a one-chunk file passes with the
    closure walk deleted.
    - **Size is stated, not discovered** — `file_offer::MAX_OFFER_BYTES` (16 MiB) is refused
      before anything is allocated, and the window prints the ceiling above the picker. The bound
      is the **offer** side, not the protocol: the pull already streams (`GET_BATCH_SIZE` = 16
      chunks ≈ 4 MiB/response), while `chunk_bytes` + `ingest_params` hold the file, its chunks
      and one CBOR envelope live at once (~4 copies) in a heap a WebView may refuse to grow.
      **Raising it is a streaming ingest, not a bigger number.** Pull reports progress per batch
      (`pull_offer_with`); the callback is **generic, not `&dyn Fn`** — a trait object makes the
      future `!Send` and the Shell's `spawn_task` requires `Send`.
    - **"Stop offering" removes the NAME, not the bytes** — `withdraw_offer` drops the manifest;
      content stays hash-addressed and a peer that already has the id can still `get` it. The
      surface says "stop offering" for that reason and must never say "delete". Both halves are
      pinned in the native proof.
    - **Offers are OPEN-POSTURE ONLY today** (buildout item 22). The grant profiles cover
      handler `local/files` on a *backend's* share and nothing else — not `system/tree` on the
      offers prefix, not `system/content` — and the app's only grant-authoring flow
      (`Action::AuthorizePeer`) targets a **backend**, so a browser peer has no surface to
      authorize a stranger to pull from *it*. Everything works because `debug_open_grants` is
      the default; `ENTITY_BROWSER_ENFORCE` fails it closed with no way to open it. Fail-closed
      is correct — what is missing is an offer-read profile plus a "who may pull from me"
      affordance on the serving side. Don't ship an enforced deployment expecting offers to work.
    - **AN OFFER IS PERMANENT, and the cost is measured** —
      `an_offer_costs_content_and_presence_bindings_that_nothing_reclaims`: a 4-chunk offer binds
      **5** entities (§6.4.2 presence binding per *ingested entity* — root and every `included`)
      plus its bytes, and withdrawal reclaims **0**. Nothing can reclaim it: `system/content`
      exposes exactly `get` and `ingest` (**no forget/unbind op**), `handle_get` serves by hash
      **without consulting the binding** (so unbinding would not even stop us serving), and the
      Worker arm has no content-remove verb at all. Routed as buildout item 21
      (`ROUTING-2026-08-16-f`). **Do not "fix" this app-side by removing the binding entities** —
      that stops us *advertising* while we keep *serving*, which is worse than either end state.
      `MAX_OFFER_BYTES` bounds one act, not the accumulation.
    - **A listing that only inserts cannot show a withdrawal.** `FsBrowseCache::apply_offers`
      **replaces** the `~`-keyed offer set (share rows untouched) and returns whether anything
      changed, and `load_offers` applies an **empty** answer too — otherwise Refresh fetches the
      shorter list, adds nothing, and leaves a dead row with a Pull button behind it. Same shape
      as the delete-reconcile lesson below: drive the change from the far side or the test passes
      on the optimistic local update (the gate withdraws from A and asserts on **B**).
    - **SAVE-STATE MANAGEMENT RIDES THIS PIPE — `src/apps/saves.rs` + the Apps window's Saves
      panel.** Back up, restore, and hand a save to another peer; the transfer is an ordinary
      file offer carrying a `SaveBundle` (set + id + opaque state), **pulled, never pushed**.
      Five things to know before touching it:
      - **A save outlives its catalog.** `list_saves` enumerates the SAVE prefix, not the catalog,
        and joins names in afterwards. Listing from the catalog would make an origin going away
        look like the user's data going away — and the saves are exactly what survives that.
      - **A bundle is identified by DECODING it, never by its filename.** The scan pulls each
        `.entitysave` candidate and decodes; a name is the offerer's word for what a file is, and
        filing a save against the wrong app on that word is the failure the envelope exists to
        prevent. A non-bundle decodes to `None` — *"that is not a save"*, not *"that save is
        corrupt"* — because a peer legitimately offers other things.
      - **Import backs up what it replaces, first.** It is the one action here that destroys a save
        without naming it (you are thinking about the incoming one), so the outgoing one is
        snapshotted on the way past.
      - **Every Send mints content the store keeps** (buildout item 21 — an offer is permanent and
        nothing reclaims its presence bindings). Re-sending the *same* bytes is idempotent (the
        manifest is keyed by blob hash); re-sending a *changed* save is a new blob. So a save sent
        after every session accumulates, and `MAX_OFFER_BYTES` bounds one act, not the total.
      - **A backup stamp is zero-padded to 13 digits** (`app_backup_path`) so the tree's
        lexicographic listing order IS chronological — unpadded, `999…` sorts after `1000…` and
        the panel lies about which snapshot is newest.
    - **SHARE WITHDRAWAL — RULED, and our empty-grants write is the CORRECT form.** Arch ruled A1/A3
      (`ROUTING-2026-08-17-f`, arch `cb5df2c`): an empty policy entry is **valid and meaningful — it is
      the withdrawal form**, and **go's 400 is the defect** (core-go moves, one deletion). Removal and
      empty-write are **distinct operations and both stay**: the empty entry is a **CEILING** at
      `request` (every non-empty request 403s) and a **union term** at §4.4 authenticate (contributes
      nothing, so the peer keeps the SHOULD floor — a withdrawal, not a ban); **removal** restores the
      `default` fallback, which on our scheme is where *public* shares live. Note the trap arch drew
      out: on go the empty state is *unreachable*, so an operator's only move is removal — **the
      conservative-looking guard produces the more permissive outcome.**
      **Our own exposure is NOT the one the ruling models, and this is load-bearing:** we never
      dispatch `capability:request` (zero hits in `src/`), so counterparts get scope from **connection-time
      grant assembly** (`assemble_inbound_grants`, core-rust `core/peer/src/connection.rs`), where our
      entry is a union term. **So our withdrawal bound is CONNECTION LIFETIME, not token TTL** — it
      takes effect at the next handshake, and a long-lived connection holds stale grants. Do not
      re-derive the `request`-path story for this app.
      **Two open follow-ons.** (1) `build_configure_params` writes only `grants` + `peer_pattern` — **no
      `ttl_ms`** — so our entries bound no lifetime; core-py already honors `policy_entry.ttl_ms` today
      (`capability.py`, the ROLE §5.6 three-way min), go and rust do not clamp at all
      (`PROPOSAL-CAPABILITY-MINT-TEMPORAL-CEILING-AND-THE-WITHDRAWAL-BOUND`). (2) **Quote no TTL number
      in any share UI until that ceiling lands** — today our only withdrawal string is
      `filetransfer.stop_offering` ("Stop offering"), which makes no temporal claim; keep it that way
      deliberately rather than by luck.
      *Historical, kept because the reasoning is still the right shape:* the divergence that surfaced
      all this was **`grants: []` accepted on rust, `400 invalid_params` on go**
      (`handleConfigure`, core-go `ext/capability/handler.go` — *"policy-entry MUST specify at least
      one grant entry"*; rust's `handle_configure` has no such guard, its `is_empty` guards sit on
      request/delegate only), routed as A1 in `ROUTING-2026-08-17-c`. **The still-true half:** an
      empty write closes the *future-mint* door and revokes nothing already minted — `revoke` is
      addressed by **token hash** (`handle_revoke` rejects a zero token) and a policy-based granter
      never sees what got minted from its own policy (`capability_path_for` scans the *handler-grant*
      namespace; rust's own comment defers user-issued caps to the **§5.1 reverse-index work**). Arch
      ruled that as **model (b)** — withdrawal is *immediate for new access*, bounded by `expires_at`
      for existing — and **corrected Q7's wording**, which had read as immediate revocation.
      **Union-on-publish stands independently of all of it** — `configure` replaces per
      `peer_pattern` however withdrawal is spelled, so the entry is always the union of live shares
      under that key, recomputed on publish *and* withdrawal.
  - **A browser peer has no listener — being reachable is an ACTIVITY it performs.** Two
    findings from that gate, both invisible in chat: (a) the establisher runs **one** negotiation
    per consultation and never retries (§7.2.1 `caller_owns_retry`, structural), so a first
    dispatch after a `meet` fails with `no transport profile for peer` — which reads like the
    peer is down. `file_offer::remote_execute` retries transport errors 10 × 1 s (never a
    returned status — a 403/404 is an answer); **do not** add app-tier "how many §6.5 failures"
    state, the bound lives in `core/peer`. (b) §6.5's *rendezvous-driven* trigger has both peers
    **meet at the key and both drive `establish_live`** — the offerer rule even has `hi` suppress
    its offer and wait for `lo`'s, which only works if `hi` is at the bucket collecting. Nothing
    in the app was doing that: a peer that only *serves* dispatched nothing, so it was never
    present, and the puller's 52 deposited offers met 0 collects. That is the specified flow, not
    a spec gap — what we lacked was **presence**, and `src/reach_keeper.rs` is it: a `meet` (or an
    `offers`/`pull`, or opening File Transfer on a peer) registers a standing intent, and while
    the kernel says that peer is not `Connected` we probe it — ~3s × 20, then ~30s, forever; a
    connected peer costs one enum compare. **It is exchange-agnostic**: it answers "is this peer
    connectable", and chat, transfer and anything later inherit it. Chat has been getting presence
    for free from its 5 Hz delivery poll — an accident, not a design. Any NEW surface where this
    peer is expected to be *reached* rather than to reach owes a
    `reach_keeper::global().want(local, remote)`, or it works only on the box where something else
    happened to be dispatching. Do NOT re-solve it with a liveness mirror or a §6.5 failure
    counter; it is neither.
    - **PRESENCE IS THE FOURTH THING `Action::ForgetConnection` MUST SWEEP, AND THE ONLY ONE THAT IS
      AN ASSERTION TO SOMEBODY ELSE.** Forget dropped the registry row, the published route (*"or
      Forget doesn't forget"*) and the dial marker — and left the reach intent, which **never gives
      up by design**, so a dismissed peer kept getting a probe every ~30s forever. `forget()` had
      **zero production callers** until 2026-08-20; it was dead outside its own test. It is
      `forget_remote(remote) -> usize` now, remote-scoped for the same reason `forget_routes_to` is:
      intent is keyed `(local, remote)` and any local peer may hold one, so a pair-scoped drop
      silently leaves the others probing (mutation-checked —
      `forgetting_a_peer_withdraws_every_local_peers_reach_intent`). **The general rule: when you add
      a standing activity that makes this peer reachable, you owe its withdrawal on Forget in the
      same commit** — the other three teardowns are bookkeeping, but presence is a *claim to a third
      party*, and an unwithdrawable one is a standing invitation to somebody the user dismissed.
    - **`connect_peer`'s "publish on the 200" does NOT port to presence, and the reason is
      structural**: a dial has a result, presence has none — success is somebody else arriving. So a
      presence-backed advertisement must be **TTL-bounded and refreshed, never durable with an
      explicit withdrawal**: `beforeunload`/`pagehide` are best-effort and a crashed or backgrounded
      tab sends nothing, so we cannot promise a withdrawal we may be unable to send. The honest TTL
      is our session — minutes, not hours. (Answered to arch as `ROUTING-2026-08-20-e` §2.1.)
    - **A stranger cannot reach us because WE cannot enumerate our inbound buckets — not because the
      key is secret.** `pair_key` sorts its arguments, so a stranger holding our id holds both inputs
      and can derive the bucket; we would have to stand at one bucket *per possible counterpart*.
      Do not restate this as "pair needs both ids so they cannot derive it" — that is false, and the
      enumerability form is what makes a peer-id-keyed fifth mode the minimal fix. Module doc carries
      it; measured against all three upstream establishers at core-rust `302b7f4`.
    **It is also the second writer of the `connections` registry**, and that fixed a real hole:
    the registry means "we have connected at least once", every target picker reads it, and only
    the manual **Connect** button used to write it — so a peer met by NAME and reached over WebRTC
    appeared in no window at all while its bytes were crossing. The keeper writes the row when a
    peer it holds intent for actually reads `Connected` (once per connect — it is a tree write).
  - **The desktop path has a gate now — `e2e_worker` Phase 14b** (the note that this file had
    "zero occurrences of file transfer" is **retired**). Browser ↔ the spawned Tauri backend over
    `local/files`: the window lists the seeded `welcome.txt`, pulls it, uploads 200 KB back, and
    the app's own read-back verification agrees. It lives **inside the display-gated Tauri block**
    because it uses that child listener — 14's own second half, sharing its `phase_gate!`; that
    gate still has exactly two tenants (14, 15.6) and this is not a third.
    - **Its last assertion leaves the app entirely: it reads the bytes off the backend's disk.**
      The test process spawned the Tauri child and shares its `HOME`, so
      `$HOME/.entity/tori-share/{name}` is just a file — compared byte-for-byte against what the
      browser sent. Every other transfer check in either suite reads *the app's report of its own
      work*; a write that is acknowledged, self-verified and yet absent would pass all of them.
      Mutation-checked by sending different bytes (red on exactly that assert).
    - **The uploaded name is stamped unique per run** — with a fixed name the previous run's file
      is already in the share, and "it appeared in the listing" passes while a broken upload
      writes nothing. (The container's `/root` is ephemeral, so this is belt-and-braces there and
      load-bearing on a dev box, where the share is the real `~/.entity/tori-share`.)
    - It also pins the **S5 write-refresh** (`handle_upload_file` enqueues `ft_refresh` for the
      initiating window, so an uploaded file appears with nobody pressing Refresh) —
      mutation-checked, and the one assertion here that is about the *app* rather than the pipe.
    - **The offer path runs on the Worker arm too — Phase 14.2** (offer → own row → Stop
      offering → row gone), deliberately **outside** the display gate, since offering is
      untargeted and needs no listener, no peer and no display. Both webrtc gates are
      main-thread, so before it every Worker-arm hop (`DispatchHandle::Worker` carrying
      `system/content:ingest`, `put_and_wait_for_cache` landing the manifest) was unproven on
      the arm this suite runs. **What it does NOT prove is the subscription** — see the
      cache-union warning above and `a_lone_file_transfer_window_lists_what_it_offers`.
    - **The cross-peer gate runs on either arm: `MODE=worker make e2e-webrtc-file`** (the rig sets
      `?worker=1` plus the `dom.securecontext.allowlist` a non-localhost origin needs). It asserts
      **the arm itself** first — Worker mode is secure-context-gated and downgrades to Direct
      *silently*, so a run that merely set the env var would prove the arm it did not exercise.
      The check reads `try_worker=true` and the absence of the fallback warning from both
      browsers, and fails closed if the needle ever stops matching. 19/19; measured
      `elected=True downgraded=False`.
- **REFRESH DELETED THE FILE IT WAS RE-ASKING FOR — the File Transfer browse cache, two causes,
  and the second one is the reusable lesson.** Reported from a real phone↔desktop run: *"I can
  download the file, but if I hit refresh it just deletes it — then Browse shows a loading thing
  and busts out."* Both halves are in `views/file_transfer/browse.rs`, both were individually
  reasonable, and the pair is what made the listing never come back:
  - **`begin_load(force)` cleared `listed`, which is the flag the RENDERER gates the whole tree
    on** (`if !output.root_listed { … return }`). So pressing Refresh tore the rows down and the
    in-flight state was a spinner over nothing. Against a peer with no `local/files` share — i.e.
    **every browser peer**, which is the whole browser↔browser transfer story — the share half
    never succeeds, so nothing ever put the flag back. A re-ask must keep showing what it has;
    `loading` is the in-flight signal and it renders *beside* the rows.
  - **`apply_offers` reported "changed" for the ROW SET only, while also repairing the
    listed/loading flags** — so the offers half quietly fixed the state and then told its caller
    there was nothing to paint. The caller repaints on `true` and on nothing else. **A "did
    anything change" return value must cover every state a render can see, not just the
    collection the function is named after** — otherwise the repair is invisible for exactly as
    long as nothing else happens to dirty the window, which on a settled screen is forever.
  - **`apply_listing` merged instead of replacing, so a file deleted on the far side never left**
    (its Pull button stayed, and 404'd). The offers half had always replaced; the share half never
    did, and the asymmetry had no reason. A removed directory now takes its cached subtree and its
    `listed` mark with it, or a directory of the same name reappearing later renders from children
    nobody would re-fetch.
  - **THE GATE THAT SHOULD HAVE CAUGHT IT WAS RETRYING.** `e2e-webrtc-file` Phase 8 clicks Refresh
    in a *loop* until the row appears — so with enough presses one lands in the ordering that
    works, and the suite was green through the whole thing. **A gate that retries cannot see a
    surface that flickers**, and "eventually correct after N presses" is not what the user has.
    Phase **8b** presses it ONCE and requires the row to survive every sample.
  - **AND THAT NEW GATE CANNOT DISCRIMINATE THE TWO CAUSES — measured, not assumed, and it is the
    fixture-blind-spot lesson again.** Restore the teardown half alone and the rig stays **green**:
    the two browsers share a podman bridge, so the offers listing lands and repairs the state
    ~10ms after the press, before the flicker outlives even a 1.4ms-per-sample loop. Over a real
    WiFi hop that listing is far slower, which is why the reported symptom was permanent rather
    than a blink. Both halves reverted together **is** red (sample #7). So: the e2e proves the
    shipped defect is gone on the shipped surface; the **per-cause** discrimination is in the
    native tests, which are the mutation-checked ones. Don't read a green 8b as evidence about
    either half alone.
- **CHAT COULD BE BOUND AND NEVER UNBOUND — a picker with no counterpart.** `render_start_picker`
  rendered only while unbound, so the first peer a Chat window talked to was the only peer it could
  ever talk to; the escape was opening a second Chat window, which is exactly how it was reported.
  **Nothing in the model was missing** — `ChatStartWith` has always accepted a second peer and
  `bind_one_to_one` has always replaced the model and the delivery — so no native test could have
  found it, and none did: what was absent was a *control*. `Action::ChatLeave` +
  `ChatWindow::unbind` + a `chat-leave` button. Four things:
  - **The generalizable rule: when a surface SWAPS one control for another on a state change, the
    new state owes its own way back.** Grep for the shape — a `if !bound { … }` / `if listed { … }`
    that hides the only entry point to a mode and renders nothing in its place.
  - **A NEW `Action` VARIANT MUST BE ADDED TO THE WINDOW-ROUTING ARM IN `app.rs`, and nothing in
    the build tells you.** `ChatLeave` compiled fine and would have been **silently dropped** —
    a Leave button that does nothing, which is the same failure class as the bug it fixes. That
    match arm's own doc comment already warns about dropped actions; it does not warn that adding
    a variant is opt-in. When you add a `{ window_id, .. }` action, add it to that group in the
    same edit.
  - **AND THE THIRD SHAPE OF THE SAME CLASS: A WINDOW HANDLER CANNOT OPEN ANOTHER WINDOW, SO A
    CONTROL THAT DOES ONE NEEDS THE DOM HALF — the Registry Browser's *Open in Site Browser* was
    inert for its whole life.** `handle_action` has no way to emit an `Action` (it holds `&Peers`
    and a dirty flag, nothing else), so `"registry_open"` registered the origin the signed binding
    carried, threw the returned peer-id away with `.is_some()`, and **marked the window dirty** —
    the Registry Browser repainted itself and nothing opened. It did real work, which is why it
    did not read as completely inert and why nobody looked. Reported from a live deployment
    against a name that resolves perfectly. Four things:
    - **`SpawnWindow` is raised from the DOM layer or the shell binding, and nowhere else** —
      grep confirms it. `dom/peer_management.rs` opening an Entity Tree is the pattern to copy.
      So when a control must open another window, the wire is `ctx.on_action(&btn, "click",
      Action::SpawnWindow { .. })`, composed with the window event if the handler must also run:
      two listeners on one element, drained from one queue **in registration order**, and that
      order is load-bearing here (origin first, window second — the other way opens a window onto
      a peer whose origin is not yet known).
    - **Put the "which window, bound to whom" rule in a free function** (`output::open_target`).
      The renderer is wasm-only, so a rule that lives there is unreachable from `make test` — and
      that is exactly the part that was wrong. **The native gate still cannot see whether the
      renderer calls it**; say so where you write it rather than implying coverage.
    - **A WINDOW'S BOUND PEER IS THE STORE IT READS, NEVER THE SUBJECT IT IS LOOKING AT — and the
      fixed button then opened an EMPTY window for a whole release because of it.** It spawned the
      Site Browser bound to `target.peer_id`, the publisher's, which reads as the obvious answer
      (*open a browser onto that publisher*) and is wrong in a way that produces a **window rather
      than an error**. Every read in that window hands its `peer_id` to `Peers` as a **selector** —
      the derived site index, `scan_local_sites`, the origins registry, prefs, provenance — while
      cached foreign content lives at `/{foreign}/sites/` in **my** store (V7 §1.4: the path's
      peer-segment says whose site it is, the selector says whose store to look in). A publisher is
      hosted by no local SDK, so `Peers::sdk_for` answers `UnknownPeer` and **every** read
      collapses to its empty value — `get_entity` → `None`, `tree_listing` → `[]`, by the §4.4
      never-fall-back-to-primary rule. The rail then says *"No external sites cached"* about a
      manifest in that very store. **Grep `SpawnWindow` for any `peer_id` that is not a local peer**;
      the audit found no other instance (Peer Management's rows are `has_context`, i.e. local; the
      palette uses `system_pid`/`selected_peer`). Gates:
      `a_resolved_name_opens_a_site_browser_bound_to_my_own_store` (the **negative** half carries
      it — the bug yields a plausible title and an empty rail, so "some peer" and "not the default"
      both pass) and `the_rail_reads_my_store_so_a_foreign_bound_window_sees_nothing`.
    - **THE FAILURE WAS NETWORK-INDEPENDENT, AND THE CROSS-REPO MEASUREMENT ROUND IS THE LESSON.**
      Every hop of the warm chain was run against production from outside this repo —
      `sites.list` · `manifest.bin` · `content/{aa}/{bb}/{hash}`, all 200 with CORS
      (`ROUTING-2026-08-24-REGISTRY-OPEN-THE-SERVER-SIDE-IS-GREEN`) — and their leading hypothesis
      was an ordering/repaint gap on our side. Reasonable, and not it: **even a re-query returns
      nothing when the selector is unknown**. Two habits. *Green on the far side of a seam narrows
      the search to your side; it does not describe what is wrong there* — and a hedged hypothesis
      handed across a repo boundary is worth answering with a **reproduction**, not with agreement.
      *And a defect that needs no network is provable natively*: the repro test seeds the manifest
      and asserts the empty rail with nothing served at all, which is why it took minutes.
    - **`find_open` keys on type AND peer.** Before the fix that was cited as a *reason* the foreign
      binding was harmless (it does not focus the boot Site Browser); it was the bug's cover. Bound
      correctly, single-instance mode now **focuses an already-open Site Browser** rather than
      spawning one, which is what exposed the dirty-signal half below — the two configurations
      behave differently and only one of them was ever tried.
    - **REGISTERING AN ORIGIN SAYS *WHERE* A PEER IS, NOT *WHAT* IT HOSTS — CLOSED.**
      `warm_peer_sites` had **one** call site (`boot_load`, over the origins
      `/entity-deployment.json` declares), so a peer learned mid-session from a signed binding — the
      entire point of the naming chain — was never enumerated. It is on the open path now **and
      verified end to end against the live registry**: pin `2KFNrGAR…@entitychurchregistry.org`,
      resolve `billslab.com`, Open → all three billslab sites in the rail in 0.6 s, each
      `cached · {host} · not verified`, clicking through to the publisher's real page. The
      *"NOT verified"* note that stood here is spent.
    - **A MID-SESSION WARM REACHED AN OPEN WINDOW THROUGH NO WATCHED PREFIX — measured 0 of 3 for a
      full minute with all three manifests in the store.** A Site Browser subscribes
      `sites/{foreign}/` only for the peers in the origin roster **at construction**, so the
      manifest write — the thing the warm exists to make — is invisible to a window that opened
      before this publisher existed, which under single-instance mode is exactly the window the
      click focuses. `warm_peer_sites` now writes the **provenance record** too, under
      `system/cache/`, which every Site Browser watches unconditionally: 0/3 after 60 s → **3/3 in
      1.8 s**. Three riders. (1) `precache_origin_sites`, its boot-time sibling, had **always**
      written both — the single write was an asymmetry, not an economy, so **when two functions do
      the same job check what the other one writes** before concluding a write is optional.
      (2) The pair is factored into `warm_site_writes` **because `warm_peer_sites` is wasm-only**
      and the pair would otherwise be a decision `make test` cannot reach; the gate's **prefix**
      assertion is the load-bearing one, since a provenance record written anywhere else satisfies
      "two writes" and restores the bug. (3) Fourth appearance of *every render input carries its
      own dirty signal* — here the input's writer is a **spawned task touching no watcher the
      reader holds**, which is the same shape as the reachability verdict and the Apps save.
    - **NO GATE IN THIS REPO DRIVES THE REGISTRY BROWSER, AND THAT IS WHY THIS TOOK THREE ROUNDS.**
      A dead button, then an empty window, then an empty window in one settings combination — each
      found by a person using it, each closed by a hand-driven WebDriver probe against production.
      The native gates cover the *rules* (`open_target`, the rail contract, the write pair); nothing
      automated covers *the click*. `make e2e-federation` is the rig that could (three containers,
      distinct addresses); a phase there is the standing next step, and until it exists **treat any
      change near this button as needing a live run**, not a green suite.
  - **Leaving is not disconnecting and not deleting**, and the tests say all three: the picker
    returns, `maintained_remotes` goes empty (which is what the release sweep reads — an intent
    left standing keeps re-dialing a conversation nobody is in), and re-binding the same peer
    shows the whole history again. The subscriptions `bind_one_to_one` added stay registered;
    dropping them needs a per-prefix unsubscribe `WindowWatch` does not have, and inventing one
    to save a repaint is the wrong trade.
  - **The gate is an e2e and has to be**, because the defect was DOM-only:
    `a_chat_window_can_be_pointed_at_a_second_peer_without_reopening_it` asserts the control
    swap in both directions and that the conversation id actually moves (it is derived from the
    participant pair, so a window that "left" but stayed bound would keep it). Mutation-checked —
    remove the button and it is red on `no way out of a bound chat`.
- **THE OFFER BUTTON HAD NO SURFACE OF ITS OWN — `src/offer_attempt.rs`, the `connect_attempt`
  lesson applied to the other silent control.** Reported from Android as *"I hit the button, it
  doesn't work, doesn't give an error, doesn't show anything"* — which is a correct report about
  the **feedback** whatever the underlying cause turns out to be. Every way an offer can fail (over
  `MAX_OFFER_BYTES`, local peer not routed, ingest refused) landed only in the Results pane at the
  bottom of the window, which on a phone is not where anyone is looking. Four things:
  - **The half the app could not see at all is the half that matters on mobile.** Between the tap
    and `Action::OfferFile` sit a file picker and an `array_buffer()` read, and both can end with
    no action ever raised — so `OfferAttempt` is a handle on `DomCtx` written from **both** sides,
    not app-private state like its two neighbours.
  - **A DOM-side write to an in-memory slot needs a DIRTY MARK, not a repaint.** Sections rebuild
    only when their `WindowWatch` says so, and this slot is in-memory precisely so no tree write
    and no subscription fires — a bare `repaint()` schedules a frame that rebuilds nothing, which
    is the same silence one layer in. The picker raises a no-op `ft_wake` window event to reach the
    handler's own `mark_dirty`; the app half pokes the `DirtyFlag` directly. Fourth appearance of
    *every render input carries its own dirty signal*.
  - **Refuse on size BEFORE reading.** `offer_file` is the authority and refuses too, but only once
    the bytes are already in wasm memory (~4 copies), and a camera-sized file is exactly where a
    phone's tab dies partway — indistinguishable from the button doing nothing. `File.size` is free.
    One wording for the refusal (`file_offer::too_large_message`), two callers.
  - **The hidden `<input type=file>` is visually-hidden, NOT `display:none`.** An unrendered input
    is one several mobile browsers decline to open a picker for — no dialog, no `change` event, no
    error anywhere. Unverified as *the* Android cause (it is not reproducible on this box), but it
    is a known class and the fix is free. **It is NOT the Android cause — see the measured entry
    below**, which was written after this one and supersedes its suspicion: a clipped input, a
    laid-out `opacity:0` input and a plainly visible input are dismissed identically. Keep the rule
    as cheap insurance for other engines; do not reach for it as the explanation.
  - **THAT FIX SHIPPED, WAS DEPLOYED, AND ANDROID STILL DID NOTHING — so the picker now goes
    through `showPicker()`, BECAUSE IT REPORTS.** Re-reported 2026-08-24 against the published
    site (checked first: the deployed build stamp carried the fix, so it was not a stale cut).
    `HTMLElement::click()` on a hidden file input is **fire-and-forget** — an engine that declines
    to open a chooser produces no dialog, no `change`, no exception and no console entry, so there
    is nothing for any surface to report and no way to tell "refused" from "cancelled". That is
    the whole reason the report is *"a broken ass button"* rather than an error. `showPicker()`
    **throws** instead (`NotAllowedError` without transient activation, `InvalidStateError`,
    `SecurityError`); `util::show_file_picker` returns `Result<(), String>` and the caller puts
    the reason on the offer status line. `.click()` stays as the fallback for pre-`showPicker`
    engines. Four things:
    - **THE RETURN TYPE IS THE FIX.** A `-> ()` API cannot be made to report; swapping to one that
      throws is what turns a silent surface into a diagnosable one. Same family as
      `parse_registry_args` returning its refusal as a *value* — and note this is the **second**
      web-sys method in this file whose signature hides a failure (`request_fullscreen` typed
      `-> Result<(), JsValue>` drops a rejecting Promise). **Check what the IDL actually returns
      before trusting a web-sys signature.**
    - **A SCRIPTED CLICK HAS NO USER ACTIVATION, WHICH MAKES THE GATE FREE.** e2e Phase 14.3 just
      presses the button the ordinary way: `showPicker` refuses, and the status line must carry
      `NotAllowedError`. The assertion is on the **reason**, not on "some text" — a ✗ with an
      empty message is the same dead end wearing a glyph. Mutation-checked: restore the bare
      `click()` and it is red with `{"ok":false,"text":""}`. (The *trusted*-click control was
      driven by hand with a WebDriver pointer sequence — no error, picker opens — the same
      activation dance the full-screen gate needs.)
    - **AND THAT WAS STILL NOT IT — THE ANSWER WAS `cancel`, AND FIREFOX FOR ANDROID IS THE CAUSE.**
      `showPicker()` did not throw either, which was the informative part: **nothing was being
      refused**. A chooser ends in `change` (a file) or `cancel` (none), and the app listened only
      for `change` — so an engine that *accepts* the request and then dismisses the chooser itself
      produces no dialog, no exception, no console entry and no status. Nothing, anywhere. Six
      things, and the method matters more than the bug:
      - **MEASURED, not inferred: Firefox for Android auto-dismisses in ~200–250 ms; CHROME ON THE
        SAME PHONE, SAME PAGE, SAME FILE WORKS.** So it is the browser — not this app, not the
        device, not the picker mechanism. Recorded in `CHANGELOG.md` under *Known limitations*.
      - **ALL NINE MECHANISMS AUTO-DISMISS, INCLUDING A PLAIN VISIBLE `<input type=file>` THE USER
        TAPS DIRECTLY.** The last hypothesis standing was that `clip: rect(0 0 0 0)` leaves the
        input "not rendered" for a Gecko heuristic, so rows 8/9 were added with a genuinely
        laid-out `opacity:0` input (proxy-clicked, and label-activated) to separate *rendered*
        from *tapped directly*. Rows 3, 7, 8 and 9 all reported AUTO-DISMISSED as well. **There is
        no app-side fix**: no hiding style, no activation path, no rendering state changes the
        outcome, so do not spend another round on the mechanism. The whole point of building the
        matrix was to be able to say that with evidence rather than after nine more guesses.
      - **`cancel` is handled now, and the INTERVAL is the discriminator**, because nothing in the
        event says whether a chooser was ever painted: `< 300 ms` is an engine dismissing itself
        and gets a message; slower is a person changing their mind and is **deliberately silent**
        — announcing that would put an error in front of everyone who ever backs out of a file
        dialog. The elapsed ms is quoted in the message so a wrong threshold is visible to the
        reader instead of hidden in source.
      - **THE INSTRUMENT WAS WRONG IN EXACTLY THE WAY THE APP WAS WRONG.** `tools/picker-probe.html`
        scored `cancel` as *"the chooser opened"*, so seven mechanisms all "passed" while every one
        of them was failing. It cost a round trip, and the operator caught it (*"sounds like it's
        automatically dismissing"*), not the probe. **When you build a diagnostic, ask what a
        FAILURE looks like through it** — this one could not represent the actual failure mode.
      - **A second probe bug hid the most interesting rows:** the timer only started when a *proxy
        button* was pressed, so the rows where the finger lands on the input itself measured from
        1970 and reported nonsense — and those were precisely the two rows that behaved
        differently. A diagnostic's own bugs land hardest on its outliers, which is where the
        signal is.
      - **THE MESSAGE NAMES A REMEDY, NOT A CAUSE.** Its first version said *"the browser has no
        permission to reach files"* — very likely wrong, since Android's chooser is the Storage
        Access Framework and needs no permission. A confident wrong cause sends someone to the
        wrong settings screen. Same rule as the WebRTC banner and the insecure-origin row: **state
        the consequence you measured, not the explanation you inferred.**
      - **Everything I changed chasing this stayed** — `showPicker` still reports genuine refusals,
        which is a real improvement even though it was not this bug. But note the shape: three
        rounds were spent changing *the mechanism* when the defect was *the event we did not
        listen for*. The probe is what broke the loop, and it should have been built first.
    - **Deliberately changed ONE thing.** The tempting move is to also drop `clip` for `opacity:0`
      and hoist the input out of the shadow root at the same time. Then a fix tells you nothing
      about which cause was real — and here the diagnostic is worth more than the shotgun, because
      the next Android run becomes decisive either way.

## Build, packaging & tree hygiene

- **D23 IS NOT A RULE ABOUT THE WORD `fetch`, AND "FIXING" THE REMAINING UNBOUNDED ONES IS A
  REGRESSION.** The discipline bounds an await that is **blocking a defined alternative
  outcome** — build-time defaults (D16), a cached shell. Two fetches in `assets/sw.js` are
  deliberately left unbounded and say so in a comment beside them: `cacheFirst` on a hashed
  asset that is *not* in the cache, and `buildScopedAsset` on a worker bin with no entry for
  this build. In both, `hit` has already ruled out a cached copy, the URL *is* the version so
  there is no older one, and an entry from a **different** build is not a fallback but the
  protocol mismatch the build-scoping exists to prevent. A deadline there has nothing to fall
  back to: it converts a slow first download of the ~30 MB main bundle into a hard failure on
  precisely the connections least able to afford one. The user-visible difference is the whole
  argument — a stall in `cacheFirst` is a first load that does not finish, which a reload
  retries (**E1**); a stall in `networkFirst` left a perfectly good cached shell unreachable
  behind an await that never returned (**E6**). They are carried by name in
  `tools/net-lint-baseline.txt` rather than by exempting their files, so the count still moves
  if a *new* one appears.
- **THE RUST-TIER DEADLINE COVERS HEADERS AND BODY, AND SPLITTING THEM RECREATES THE BUG ONE
  STEP LATER.** `net::fetch_text_bounded` deliberately does not return a `Response`. An origin
  may answer `200`, hand over headers, and then never send a body — so a helper that bounds
  only the header phase and lets the caller `await resp.text()` has moved the unbounded await
  rather than removed it. One call, one deadline, both phases; the `AbortController` signal
  covers the body stream too, which is what makes that possible. The `DeadlineGuard` clears
  the timer and drops its closure on **every** exit path including the early `?` — a leaked
  `Closure` here is per-boot and permanent (this repo never calls `Closure::forget()`).
- **A DOC COMMENT THAT RUNS INTO THE NEXT ONE IS SILENT, AND THIS IS THE SECOND INSTANCE.** The last
  session caught itself splitting `render_meet`'s doc; this one found `meet_verb`'s doc block
  already merged into `net_verb`'s at HEAD — so `meet_verb` had no documentation and `net_verb`
  carried a paragraph about a different verb, with clippy silent (two `///` blocks separated by a
  blank line or a `//` comment still both attach to the next item). It also happened *during* this
  session: inserting a helper between `connector_verb`'s doc and `connector_verb` reattached the
  doc to the helper, and clippy's `empty_line_after_doc_comments` was the only hint. **When you add
  a function next to an existing one, check which item its neighbour's doc is now attached to.**
- **CACHE HEADERS ARE OPT-IN TO IMMUTABLE, NOT OPT-IN TO MUTABLE — the old rule put a ONE-YEAR
  cache on the file carrying the registry pin.** `tools/cors-serve.py` is the reference
  implementation of the runbook's header policy and the thing an operator copies into a CDN config;
  its rule was *"`published-root` and `.list` are mutable, everything else is
  `max-age=31536000, immutable`"*. A deployment root holds **four more mutable files nobody
  enumerated** — `entity-deployment.json` (the pin, the home site, the whole posture),
  `transport-profile`, `index.html`, `sw.js` — and each inherited the year-long immutable default.
  Change your registry pin and no returning visitor sees it; ship a new build and nobody loads it.
  **Invisible locally** (a fresh container has no cache) and not noticeable the day you set it —
  you notice the first time you need to correct something. Now: immutable **only** for `/content/`
  and for `name-<8+ hex>.wasm|.js` (trunk stamps the hash into the name, so a rebuild is a new
  URL); everything else `no-store`. The same rule is duplicated in `named_site.rs`'s test server
  so the config an operator copies and the server our tests trust cannot disagree.
  - **THEY DISAGREE. Audited 2026-08-25: the rule has FOUR expressions and no two are the same** —
    `cors-serve.py` (`"/content/" in path`, on the raw target *including the query*),
    `app_server.rs` (`starts_with("/content/")`, query stripped), `named_site.rs`
    (`contains("content/")`, **no leading slash**), and the Cloudflare recipe in
    `PUBLISHING-QUICKSTART.md` §6.2 (`starts_with`). The hash tests differ too: two require the hex
    run to reach the extension, `named_site.rs` does not. **The sentence above was written as a
    guarantee and was never an enforced one** — each implementation passes its own tests, which is
    exactly why the drift is invisible. Live consequences both directions: a site with a `content/`
    directory (Hugo, Zola and Lektor all name their source tree that) gets **mutable HTML pinned for
    a year** under the substring rules, and a `--prefix` deployment's entire content store —
    `/{prefix}/content/…`, which `make federation` really emits — is **under-cached** under the
    prefix rules. Measured zero live instances of the first across every published tree on the box;
    92 files of the second in `dist-federation`. Full audit:
    `docs/status/REVIEW-2026-08-25-cache-classification-and-the-service-worker.md`.
    **Two expressions of one rule that can disagree eventually do — and four of them already have.**
  **The general shape: when a policy enumerates the exceptions and defaults the rest to the unsafe
  value, every artifact added later inherits the unsafe value silently.** And note what could not
  catch it — every gate we own runs against a server with no cache in front of it.
  - **AND EVERY `make` SERVE TARGET BYPASSED THAT FILE ENTIRELY, WHICH IS WHY LOCAL RELOADS SHOWED
    A STALE BUILD.** `RUN_SERVE` ran `python3 -m http.server`, which sends **no `Cache-Control` at
    all** — and "no `Cache-Control`" is not "uncached": a browser then applies **heuristic
    freshness** off `Last-Modified` and serves the shell from cache without revalidating. Reported
    from the outside as *"why isn't it detecting that it changed — I have to use a private
    window"*, which is a correct bug report and reads like a service-worker problem. `RUN_SERVE`
    now runs `tools/cors-serve.py` (it takes `(directory, port)` and is already
    `ThreadingHTTPServer` on `0.0.0.0`), so `serve` / `build-serve` / `site-serve` / `dist-web` all
    get the documented policy: `no-store` for the mutable shell, `immutable` only for
    `/content/` and hash-named bundles. **The reference implementation of a policy is worth
    nothing while the thing everyone actually runs does not call it** — same shape as a documented
    invariant with no enforcement point, one layer over.
- **Publish outputs must NOT live under `dist/` — `trunk` wipes it [F10].** `make federation`
  defaulted to `dist/federation`, so publishing a federation and then running `make wasm` deleted
  the federation *silently*; the next serve 404s and reads as a broken publish, not as a missing
  directory. Now `dist-federation/` (and `dist-registry/`), matching the `dist-publish` /
  `SERVE_DIR`-under-`/tmp` precedent the `site-serve` block already set for exactly this reason.
  The federation path must stay **inside the repo** (the container's only bind mount), so `/tmp` is
  not available to it the way it is to `SERVE_DIR`.
- **A PUBLISH FLAG WITH NO `make` PASSTHROUGH IS UNUSABLE ON A PODMAN-ONLY HOST — `REGISTRY_PIN`,
  and it is the THIRD instance of audit F3's class.** `publish --registry-pin=PEER_ID[@ORIGIN]`
  seeds the §7.4 registry pin into `/entity-deployment.json`; the `site` target enumerates its
  flags explicitly and did not carry it, so on a podman-only host the only route was a bare
  `cargo run` — which is exactly the hole `make registry` was created to close on the emitting
  side, kept on the consuming side. Reported from outside this repo, and worked around by **editing
  the emitted JSON**, which is the harm: `parse_registry_pin` refuses a peer-id a consumer could
  never use, on the operator's machine where the refusal can be read, and a hand-edit skips
  precisely that check so the failure resurfaces at a visitor's browser as *"this registry is
  down"*. Four things:
  - **Added to every target that emits a deployment config** — `site`, `site-serve`,
    `tauri-bundle` (`site-dist` inherits through the sub-make, verified rather than assumed) — and
    deliberately NOT to `site-bare`, which emits no config at all. A per-target fix is how the
    next one ships unguarded; that is the `CHECK_IN_TREE` lesson, one target family along.
  - **The flag audit found NOTHING ELSE**, and that is worth recording so nobody re-runs it:
    every other `publish` flag has a make route, `--ingest-games` is an **alias** for
    `--ingest-apps` (not a second capability), and `registry --peer-id` / `--issued-at` are
    reachable because `make federation` runs the script *inside* the image, where cargo exists.
  - **`make site-dist REGISTRY_PIN=… ` THEN FAILED ITS OWN VERIFY PASS — an emit-time guard was
    gating a read-only mode.** The target publishes and then re-runs `site` with `VERIFY=1`,
    inheriting the caller's knobs; `--verify` writes nothing, but the pin guard fired anyway and
    the whole target failed *after* the tree was correctly written. The other emit guards never
    surfaced it because they key on `--deployment-config` (which that pass does not set) — the pin
    guard keys on the **pin**, which it does. Fixed structurally: the `verify_only` return now sits
    **above** every emit-time guard, so this cannot recur for the next flag either. Gate:
    `verify_is_not_gated_by_the_emit_time_rules_it_cannot_violate`, both directions.
  - **Found by RUNNING `site-dist`, after `site` had already been tested end to end.** A
    two-step target is a different caller, and the set of knobs it forwards into a read-only pass
    is not something anyone enumerates. When you add a flag, run the *composed* target too.
- **`make federation-vectors` DELETED THE CORPUS README — the file its own docs tell you to read
  first.** The recipe is `rm -rf $(FED_VECTORS)` then republish, and the hand-written `README.md`
  lives *inside* that directory, so the documented "regenerate with the same command" workflow
  destroyed the only explanation of what the bytes are — including its own Regenerating section.
  It is carried across the wipe now. **The general shape: when a regeneration owns a whole
  directory, anything hand-written in that directory is on a timer**, and the failure is silent
  because the regeneration succeeds. Also: the completion message now states the expected diff
  (5 `published-root`s + their signature/blob renames, from an upstream wall clock), because "a
  diff means the emitter changed" plus 40 churning files is a warning nobody can act on.
- **`.gitignore` ENUMERATED THE `dist-*` DIRS ONE AT A TIME, SO THE NEXT ONE WAS COMMITTABLE BY
  DEFAULT — and `725bc20a`, a CHAT FIX, swept in 18,408 files / 972,606 insertions.** A 114 MB
  `dist-all/` publish tree and an 11 MB frozen SPA shell (`dist-spa-pristine/`), neither of which
  any target regenerates under those names, neither referenced anywhere in the repo (grepped:
  Makefile, scripts, docs, Rust, workflows — zero hits). It is the **cache-header lesson exactly**:
  *a policy that enumerates the exceptions and defaults the rest to the unsafe value gives every
  artifact added later the unsafe value, silently.* Now `/dist-*`. Three riders:
  - **`dist-spa-pristine` is DELETED**, at meta's ask: a committed 2026-08-20 shell that nothing
    regenerates can only get staler while cuts built on it look current, and it already cost a
    session — a cut was hunted for app bugs fixed 62 commits earlier. Their pipeline's
    `--reuse-spa` branch reads it and `die`s with an actionable message when it is absent; their
    `--spa-from vN` (seed from a *named* previous cut) replaces it.
  - **`dist-all` is UNTRACKED, NOT deleted** (`git rm -r --cached`): there were live servers on
    :8300/:8310 serving it when this ran. Removing it from the index stops it growing and stops it
    reading as source; it does **not** shrink history, and saying otherwise would be the overclaim.
  - **Neither is recoverable-by-rebuild under its own name** — `make site-dist` reproduces the
    shape, not the bytes. History holds them; nothing else does.
  - **AND THE FIX ABOVE MISSED EIGHT MORE, BECAUSE `.gitignore` NEVER APPLIES TO AN
    ALREADY-TRACKED PATH.** `/dist-*` was the right rule and could not reach
    `dist-{billslab,entity-church-foundation,entity-core-protocol,prodtest}` or their four
    `dist-review-*` near-duplicates, all committed by that same `725bc20a`: they were already in
    the index, so the glob was inert against them and the class was reported closed while **12,488
    of 13,469 tracked files (93%) were rendered output** — 354.2 MB of HEAD's 367.3 MB. Untracked
    2026-08-23 (`cb47662d`); tracked content is now **981 files / 13.1 MB**. Four things:
    - **EVERY SIGNAL POINTED AWAY FROM IT, which is why one grep did not find it.** `git status`
      reads clean (they are tracked and unmodified) and **`git check-ignore` answers NOT IGNORED**
      — it skips tracked paths unless you pass `--no-index` — so the one command you would reach
      for reports the ignore rule as *missing* rather than as *inert*, which sends you to edit
      `.gitignore` again. The question that actually answers it is
      **`git ls-files -c -i --exclude-standard`**: tracked files the ignore file covers.
    - **The enforcement point is `tools/tree-hygiene.sh`, in `make lint`** — that one command,
      mutation-checked both ways, rolled up to directories. Deliberately **not** a path list: a
      list has to be maintained and the next artifact directory inherits the unsafe default the
      same way, which is precisely how this recurred *inside its own fix*. The general invariant is
      *the ignore file and the index must agree*, and it needs nothing kept current.
    - **The working-tree figure is not the history figure — 354 MB of files is ~58 MiB packed**
      (content-addressed dedup, and the four `review-` trees are near-duplicates of the other
      four). Quote the packed number when anyone weighs a history rewrite; the big one overstates
      the cost by ~6×. *(An earlier draft said 53.6 MB, summed from `%(objectsize:disk)` per blob —
      close, but the honest way to get this number is to actually rewrite a throwaway clone and
      `gc` it, which is what produced 58.02.)*
    - **`master` never carried any of this** (0 tracked `dist-*` on every branch but `dev`), so
      nothing ever reached GitHub — the public repo is an **orphan snapshot** (`bf4e6ee8` is a root
      commit with no parents), not a merge of `dev`, so `dev`'s objects were never going to travel
      on their own.
    - **THE HISTORY WAS THEN REWRITTEN, 2026-08-23, AND THE SHAs ABOVE ARE THE NEW ONES — so
      `725bc20a` does NOT show the 18,408 files this entry opens by describing.** That is the point
      of the rewrite, and it is recorded here because the alternative is a future seat reading this
      paragraph, running `git show`, finding an ordinary chat fix, and concluding the entry is
      folklore. The measurements above were taken *before* it. Operator-authorized, on a branch
      only this seat works; `dev` went **18,959 objects / 64.11 MiB → 9,061 / 6.09 MiB** with the
      tip tree byte-identical, all 674 commits, messages, authors, committer dates and 35 DCO
      sign-offs preserved. Full record + the old→new SHA map:
      `docs/status/STATUS-2026-08-23-the-rendered-sites-are-out-of-devs-history.md`. **Older
      `docs/status/` snapshots still cite the pre-rewrite SHAs and were deliberately NOT edited** —
      a published snapshot is immutable (ADR-0009/0018); resolve them through that map.
    - **Standing lesson, and it is this file's most repeated shape in a new place: a cleanup
      commit's own report is a claim about what it touched, not about the class it names.** The
      previous session fixed the rule, fixed one instance, and wrote "closed" — correct on both
      halves and wrong on the conclusion. **After ratcheting a rule, enumerate what the OLD rule
      let through and check each one**, because a new rule is not retroactive over anything git
      already tracks.
- **THE RELEASE NOTES ARE GENERATED FROM `CHANGELOG.md`, SO A STALE ENTRY THERE IS A FALSE
  STATEMENT TO A DOWNLOADER — NOT AN INTERNAL NOTE.** `.github/workflows/release.yml` awks that
  version's section out and appends `.github/release-notes-footer.md`; whatever the two say IS the
  release page. Measured on the 0.8.2 section, 2026-08-23: it had lost **six** shipped features,
  claimed **571** locale keys against a measured **701**, and listed *"no save history and no way
  to restore an earlier state"* as a known limitation while `apps::saves::list_backups` had been
  returning timestamped restorable backups for days. Every one of those is a sentence a stranger
  reads about software they just downloaded. Four things, each of which is its own trap:
  - **A DOCUMENT THAT ENUMERATES ARTIFACTS GOES STALE SILENTLY, AND THE FOOTER IS THE WORST CASE.**
    Its download table listed five installers by filename. If a release ships fewer than the table
    names — which is exactly what a reduced or partially-green matrix does — the notes advertise
    downloads that 404, with the *permanent-link* examples pointing at the missing ones. Same
    family as the cache-header and `.gitignore` entries above: **a hand-maintained list beside a
    thing that varies drifts, and the drift is invisible on the machine that writes it.** The
    long-term fix is to generate the table from what was staged, as the `Version manifest` step
    already does; until then a removal carries a comment naming what must restore it.
  - **A RELEASE-FACING LINK MUST POINT AT A PATH THAT EXISTS IN THE RELEASED TREE.** The footer
    linked `docs/RELEASE-READINESS.md` — a file that does not ship — so it was a 404 aimed at
    exactly the reader that section is written for. **Not every file here reaches a release**, so
    check the target against a released tree before linking it from `README.md`, `CHANGELOG.md`,
    `.github/release-notes-footer.md` or `docs/STATUS.md`. Those four, and `AGENTS.md`
    itself, are read by people who do not have this checkout.
  - **THE VERSION LIVES IN THREE FILES AND `check_dist_version` REFUSES A MISMATCH** —
    `Cargo.toml`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`. It fires before any build
    work, which is the right place; just know that a version bump is three edits, not one.
  - **A "WE CANNOT DO X" LINE IS THE ENTRY MOST LIKELY TO GO STALE, BECAUSE ITS EXPIRY IS SOMEBODY
    SHIPPING X — and a fifth one was found on 2026-08-24, two days after this entry was written.**
    The 0.8.2 section said **twice** that TURN was unsupported (*"there is nowhere to carry a
    credential yet"*, *"refused for want of a credential channel"*), while
    `Connector.relay`/`.relay_username`/`.relay_credential`, `session_config::parse_relay` and
    three boxes on the connector row had all shipped. **The product contradicted its own release
    notes out loud**: typing `turn:` into the reflector field answers *"put it in the Relay field,
    not here"*. So when you review this file, grep the section for *no way to · nowhere to ·
    is refused · not yet · cannot* and check each against the tree — a features list drifts by
    omission, which is recoverable, but a limitations list drifts into **falsehood**.
  - **AND IT CAME FROM A CODE DOC COMMENT, WHICH IS THE PART TO CARRY.** `parse_ice_urls`' doc had
    said *"there is nowhere to put them here yet … carrying credentials is a later, additive
    shape"* — a correctly-flagged concession with an expiry, written exactly as this file asks —
    **and nobody spent it**, so the note outlived its truth and was then copied verbatim into a
    user-facing document. A stale comment is an internal cost right up until prose is written from
    it. **State a concession's expiry where you make it, then go spend it** (the standing rule),
    and when you land the thing that spends one, grep the codebase *and* `CHANGELOG.md` for the
    sentence it retires.
  - **DO NOT REASON FROM A `release/*` BRANCH IN THIS REPO — `dev` IS THE TREE.** A seat found
    `release/0.8.2` carrying stale release notes, wrote it up as a pre-tag item and told the next
    session to merge `dev` into it. The operator's correction was blunt and is the rule: that
    branch is not real and is not what ships. **A branch whose name asserts a role is not evidence
    that it holds one** — the same shape as this file's standing warnings that a `cfg` line and a
    constant name are not the code path, arriving through a *branch name*. Ask the operator which
    ref ships; do not infer it from `git branch -a`.
  - **A DOC CAN GO STALE IN THE DIRECTION THAT UNDERSTATES THE PRODUCT, AND THAT IS THE COSTLY
    ONE.** `GUIDE-REPUBLISH-AND-INCREMENTAL.md` §1 told publishers *"`entity-browser publish` emits
    none of that, and our browser reads none of it (zero references in `src/`)"* about signed roots
    — while `signed_root::RootProjector` runs at `publish.rs:1587` and `SignedSession` /
    `signed_fetch.rs` are the consumer. It had already been corrected once (2026-08-17) and went
    stale again, which is what a dated correction buys you and no more. Two things: **the honest
    rewrite was a SPLIT, not a deletion** — our publish *is* signed and the naming chain *does*
    verify it, while the Site Browser's navigate path still does not, so a single sentence either
    way was going to be false in one direction; and **understating is not the safe direction** — it
    told publishers their content was unsigned, which is the one claim that would stop someone
    pinning a key. Run the `no way to · nowhere to · is refused · not yet · cannot` sweep over the
    **guides**, not just `CHANGELOG.md`.
  - **DO NOT PIPE A COMMAND YOU ARE JUDGING THROUGH `tail` — the pipeline's exit status becomes `tail`'s.** `make test |
    tail -40` in this session reported **exit 0 while showing 2 of 16 binaries**, and the truncation
    is what made it obvious; had a test failed it would have reported exit 0 just the same, silently.
    Redirect to a file and read the file (`make test > log 2>&1; echo $?`). Same family as
    `Stdio::null()` on a helper server's stderr: **never put a filter between a gate and its verdict.**
  - **THIS FILE IS PUBLISHED — WRITE IT FOR A PROJECT DEVELOPER, NOT FOR US.** `AGENTS.md`,
    `AGENTS-STANDARD.md`, `METHODOLOGY.md`, `README.md`, `CHANGELOG.md` and `docs/STATUS.md` all
    ship to the public repo. **Nothing about how we run the project goes in any of them** — no
    absolute paths off this machine, no coordination-tree or staging directory names, no internal
    tooling, no release-pipeline mechanics, no other-team routing. Two of those leaked into this
    file and had to be cut. **`docs/status/` is the home for all of it** — that directory is
    stripped and never publishes. The test before you add an entry here: *would this help someone
    who just cloned the repo and wants to change a window?* If it is about our operations instead,
    it goes in `docs/status/`.
- **A CONTAINERIZED PUBLISH WITH AN OUT-OF-MOUNT `OUT` PRINTED SUCCESS AND WROTE NOTHING — fixed by
  `CHECK_IN_TREE`, and the rule had been written down for months.** Every publish verb runs in the
  image, whose only bind mount is `$(PARENT)`. `make registry REGISTRY_OUT=/tmp/x` emitted into the
  *container's* filesystem, the chained `--verify` ran in a **second** container that saw an empty
  directory and reported *"this tree is POINTER-TRUSTED … (Not a defect)"*, and make exited **0**.
  Measured: exit 0, no such directory on the host. `REGISTRY_OUT=/srv/www/registry` — publishing
  straight into a webroot, the obvious thing an operator tries — fails exactly this way. Three
  things: **(1) the guard uses `readlink -m` against `$(PARENT)`**, not a `..`/absolute-path
  heuristic, so a sibling repo genuinely inside the mount is still allowed; **(2) it is on all four
  publish targets** (`site`, `site-bare`, `registry`, `federation`) — a per-target fix is how the
  next one ships unguarded; **(3) the failure mode is the operator-surface shape this repo keeps
  meeting** (F3, F9, the `--bind=` parse): *the one person who could fix it is told it worked*. The
  rule was already in `FED_OUT`'s own comment — *"must stay INSIDE the repo"* — and enforced
  nowhere. **A documented invariant with no enforcement point is theater; when you write one into a
  comment, ask what would fail if it were violated, and if the answer is "a success message", go
  build the check.**
- **`make site-dist` IS THE UPLOADABLE WEB TREE, AND UNTIL NOW THAT ARTIFACT HAD NO NAME**
  (handoff §3). `make site` emits the **content half only** — its root `index.html` is a redirect to
  `/sites/` and there is no wasm — so uploading it to a bucket root **replaces the live SPA with a
  redirect page** and orphans every app bundle (`.bin` under `{peer}/apps/**`, reachable only
  through the SPA; nothing in the static HTML projection links to them). The composition existed in
  two places and neither emitted an uploadable tree: `site-serve` **serves** it, `tauri-bundle`
  **embeds** it — so the only written record of the rule was a comment on a **desktop** target,
  which is not where anyone doing a web release is reading. Three things: **the ordering is
  load-bearing** (`wasm-release` FIRST — trunk wipes its dist dir — THEN publish into it, which
  cleans only `sites/`, `content/`, `{peer}/`); it is **two sub-makes, not two prerequisites**,
  because prerequisites are order-independent under `-j` and an order is the one thing this target
  exists to guarantee; and **`DEPLOY_CONFIG` defaults to 1 here**, because without
  `/entity-deployment.json` the SPA boots to its own seed and every published site appears
  **missing** — a production upload needs it and nothing else enforced that. Output is
  `dist-site/`, deliberately **not** under `dist/` (trunk wipes that — the F10 lesson).
- **`INGEST`/`APPS_DIST` MAY POINT ANYWHERE NOW — every publish target stages them** (handoff §2).
  The publish runs in a container bind-mounting only `$(PARENT)`, so an external source was
  invisible and the real sources (papers, entity-apps — in a sibling meta tree) had to be
  hand-copied first. `tauri-bundle` had solved it for itself; `stage_publish_sources` is that lift,
  now shared by `site` / `site-dist` / `site-serve` / `tauri-bundle`. Note **why this is not
  cosmetic**: a manual copy step is a step you can skip, and skipping the `APPS_DIST` half is
  precisely the omission the entry above shows deleting a domain's apps tree. Staging is
  unconditional when the variable is set (one predictable behaviour beats "it worked because my
  checkout happened to be in the right place") and **skipped under `VERIFY=1`**, which reads only
  the output tree. Verified with an `APPS_DIST=/tmp/…` genuinely outside the mount.
- **A GREEN BUILD CAN BE A CACHED BUILD — check the artifact timestamps before quoting a gate.**
  Measured 2026-08-18, and it burned a whole session's reporting: a sibling repo (`entity-core-rust`)
  gained an uncommitted `core/peer` change adding a 6th `DispatchCeiling` arg to `make_execute_fn`.
  `make test`, `make lint` and `make wasm` all stayed **green for hours** because cargo/trunk never
  rebuilt `entity-sdk` — until `make e2e-worker`'s *different feature set* (`--features demo-apps`)
  forced a rebuild and every target broke at once. `dist/*.wasm` were timestamped **before** the
  upstream edit and `check-dist` only asserts the files *exist*. **The tell was a 2.29s "Finished"
  on a build that should take minutes.** So: `ls -l dist/*.wasm` after a wasm build you intend to
  quote, and treat a suspiciously fast build as unverified. This is the stale-build-state trap the
  worktree section warns about, arriving through the *cache* instead of through prose.
- **Baking content into the Tauri desktop app** = `make tauri-bundle` (NOT `make
  tauri`, whose `wasm-release` wipes `dist/`): it publishes sites/apps into `dist/`
  then embeds, served same-origin by the WebView. A returning app's **durable
  config wins over the baked `entity-deployment.json`** (persisted > fetched >
  build-time), so re-testing a bundle needs a fresh profile (clear
  `~/.local/share/<app-id>`). Full story: Deployment Guide §8.1.
- **THE 17 MB WORKER BUNDLE WAS RE-DOWNLOADED ON EVERY LOAD, ONCE PER WORKER — FIXED 2026-08-22
  (`assets/sw.js`, `WORKER_ASSET` + `buildScopedAsset`), gated, and mutation-checked both ways.**
  Measured at the wire: **3 requests across 3 loads → 1**. It was a product bug every visitor paid
  *and* the root cause of the e2e's intermittent `Navigation timed out after 60000 ms`, which landed
  on a different phase every run (19, 20, 21 all seen) and read as suite flake for three sessions.
  Six things, and the two traps in the middle are the ones that will cost the next person:
  - **THE FIX IS NOT THE ONE THIS ENTRY USED TO RECOMMEND, and the recommendation was wrong on both
    halves.** It said *"hash the worker bundle's filename … do not reach for a SW-side change without
    trying it first"*. Hashing is not available: trunk's `data-type="worker"` pipeline emits fixed
    filenames **by design** (the loader references them by a stable URL), `assets/entity-worker-loader.js`
    hardcodes `importScripts('/entity-worker.js')` and `module_or_path: '/entity-worker_bg.wasm'`,
    and that file is `copy-file` — trunk does not template it. And the SW-side change it warned
    against was *stale-while-revalidate*, whose trade (a deploy's worker arrives one load late) is
    real — but that is not the only SW-side answer. **The worker is cache-first keyed on the BUILD
    ID: the main bundle's hash, which trunk does stamp and index.html does name.** Same build ⇒ zero
    bytes; new build ⇒ a miss ⇒ fetched exactly once. It carries no staleness trade at all, and it
    is *stronger* than hashing the worker's own bytes would be — the key IS the other half of the
    runtime, so main bundle and worker cannot disagree by construction.
  - **TRAP 1 — `cache.match(url, {ignoreSearch: true})` RESOLVES IN INSERTION ORDER, AND IT SERVED A
    STALE WORKER.** The first version cached navigations under their own URL (`/?worker=1`) and read
    the build id back with `ignoreSearch`. The cache also holds `/` from `CORE_ASSETS` at **install**,
    which is never updated — so the match kept returning the install-time shell, the build id could
    never move, and a deploy handed the OLD worker to the NEW bundle: an
    `entity-wasm-worker-protocol` mismatch, i.e. **worse than the bug being fixed, and silent**.
    Navigations are now cached under one canonical `/`, awaited, and the read is an exact match.
    Generalize it: `ignoreSearch` is a *lookup* convenience and never a *freshness* guarantee.
  - **TRAP 2 — THE OBVIOUS IN-BROWSER ASSERTION IS VACUOUS, AND ONLY MEASURING SAID SO.** The
    natural gate is `performance.getEntriesByType('resource')` inside the worker and asserting
    `transferSize === 0`. **Firefox zeroes `transferSize` AND `encodedBodySize` for every response a
    service worker supplied**, whether the SW hit its cache or pulled 17 MB — measured on a load that
    demonstrably fetched and one that demonstrably did not, both reporting
    `transferSize=0 encodedBodySize=0`. A gate built on it passes in both states and looks fine. The
    loader line written to carry it was **deleted rather than kept**, because a diagnostic that reads
    the same in both states is worse than none. **The server's request log is the only honest account
    of bytes on the wire**, so `DistServer` now drains and counts it (which also closes a real gap —
    that stderr used to be read only on the immediate-exit path).
  - **THE DEPLOY PATH NEEDS ITS OWN PROBE, AND IT IS WHAT CAUGHT TRAP 1.** *"Fetched once across 3
    loads"* was green while the stale-worker bug was live; the failure only exists **across** a
    build change. The probe swaps the main bundle's hash **inside one browser session** (so the SW
    cache survives, as a returning visitor's would) and requires `1,0 / 1,0`. Anything that changes
    worker caching owes that shape, not just a repeat-load count.
  - **Gate:** `the_worker_bundle_is_fetched_once_per_build_not_once_per_load` (standalone, ~3.5 s).
    Mutation-checked: restore the pre-fix `sw.js` → red on `3 != 1` with the cause in the message.
  - **Still open, and NOT fixed by this:** an origin with **no service worker** — a plain-http LAN
    host, i.e. the two-machine M1 path — has no SW cache, so the worker is re-fetched every load by
    the HTTP cache rules, where `tools/cors-serve.py` and `app_server.rs` both mark it `no-store`.
    Worker mode cannot work on that origin anyway (OPFS is secure-context-gated), which raises a
    second unmeasured question: whether we download 17 MB and then fall back to Direct. **Measure
    before claiming either way.** The historical diagnosis follows, kept because the shape recurs.

  Measured across three SW-controlled reloads of the same page, before the fix:

  | asset | size | fetches |
  |---|---|---|
  | `entity-browser-<hash>_bg.wasm` | 29.5 MB | **0** — hashed, served from the SW cache |
  | `entity-worker_bg.wasm` | 17.1 MB | **3** — one per load |

  The mechanism is entirely in `assets/sw.js` and needs no theory: `HASHED_ASSET` is
  `/-[0-9a-f]{8,}(_bg)?\.(js|wasm)$/`, trunk hashes `entity-browser` but emits the worker bundle as
  a bare `entity-worker_bg.wasm`, so it misses the regex, takes `networkFirst`, and `networkFirst`
  does `fetch(req, { cache: 'reload' })` — which **deliberately bypasses the browser's HTTP cache**
  (the comment there explains why: an unconditional fetch is what stops a 304 serving a stale
  shell). Correct for a 2 KB shell; catastrophic for a 17 MB bundle. Four things:
  - **It multiplies by the number of WORKERS, not pages.** Boot spawns one; every persisted
    `Backend*` peer respawns another. By e2e Phase 16 there are two, so a single navigation pulls
    **~51 MB** — while the navigation itself is *also* on the `networkFirst` path, competing with
    them. That is how a `goto` that takes **0.04 s from a fresh session** blows past a 60 s
    pageLoad bound on a busy box, and why it is intermittent and lands wherever it lands.
  - **THE SIGNATURE THAT LOOKED LIKE A RACE WAS THIS.** `wait_for_boot`'s log dump showed two
    `[entity-worker] loader script start (t=0ms)` and only one `importScripts done`, which reads
    exactly like a concurrency bug in worker init. It is not — both workers were pulling 17 MB
    unconditionally and one had not finished. **A fix built on that reading made it worse**:
    serialising the spawns (boot first, then a `WORKER_SPAWN_BUSY` gate) left the first respawn
    holding the gate on its own download and the second never starting, taking Phase 16 from 3 SDKs
    to 2. Reverted. *The two-workers-one-import log is a symptom of bandwidth, not of ordering.*
  - ~~**The real fix is to hash the worker bundle's filename.**~~ **Withdrawn — trunk emits worker
    filenames fixed by design and the loader hardcodes them; see the first bullet for what shipped
    instead.** Kept visible because it is the fourth stale claim this file has carried that survived
    by being read rather than run: the recommendation was written from the *shape* of the problem
    without checking whether the mechanism it depended on was available.
  - **Production was affected, not just the rig.** A release build is smaller (`opt-level=z` + LTO +
    `wasm-opt -Oz`) but the ratio was unchanged: every visitor re-downloaded the whole worker bundle
    on every load, once per worker, forever, and no HTTP cache could help because the fetch opts out
    of one. **MEASURED 2026-08-24 (`make dist-web`): the worker bundle is 3,846,313 bytes release
    against 17,115,047 debug — 4.45×.** So the fixed bug was costing a real visitor ~3.8 MB per load
    per worker, not 17 MB; two workers by e2e Phase 16 made it ~7.7 MB, not ~51 MB. **Quote the
    release figure when describing production and the debug figure when describing the rig** — this
    entry cited the rig's numbers for both, which overstated the user-facing cost by 4×.
- **Release mode: Mode 1 only** (ADR-0023). This repo ships a **binary/app**
  release — Tauri installers for the 5 table-stakes targets + the web-class wasm
  bundle — and publishes **no package to any registry**, so it does not define
  `publish`. The class tool is **Tauri's bundler, not GoReleaser** (which cannot
  build a `.deb`/`.dmg`/`.msi`, and whose prebuilt-artifact path is a paid
  feature): `docs/adr/0002-mode-1-release-via-tauri-bundler.md`.
  **The platform matrix has ONE home — `.github/workflows/release.yml`.** Never
  add a `dist-<platform>` target and never a `make release`; a release is
  `git tag vN` on green master (ADR-0015). Operational how-to:
  `docs/RELEASE-READINESS.md`.
- **Know exactly what cross-builds, because two of the three failure modes are
  SILENT.** Measured against `tauri-bundler 2.9.4`: Windows **NSIS
  `_setup.exe` cross-builds from Linux** (`PackageType::Nsis` is deliberately
  un-gated — cargo-xwin + clang/lld + makensis, all in the image; proven here).
  Windows **`.msi` does not** — `#[cfg(target_os = "windows")]`, and off Windows
  the request falls to `_ => log::warn!("ignoring msi")`, so you get a release
  **quietly missing a file** rather than an error. **macOS does not** — `mod
  macos` is itself `#[cfg(target_os = "macos")]`, so the `.app`/`.dmg` bundlers
  are compiled out; `make dist DIST_OS=macos` refuses immediately rather than
  producing nothing. Cross is a variable (`DIST_OS=…`), never a new target.
- **`make publish` is GONE — it is `make site`** (`publish-bare`/`publish-serve`
  → `site-bare`/`site-serve`). `publish` is a **reserved** fleet-wide verb
  (ADR-0023 Amendment 1) meaning "push a package to its native registry"; the old
  names remain for one release as stubs that fail with a pointer. The **app CLI
  is a separate namespace and did NOT change**: `entity-browser publish <dir>`,
  `PUBLISH_DATA_DIR`, and `PUBLISH-INGEST-FORMAT.md` all still say publish.

## Testing & the gates

- **THE SERVICE-WORKER TIER HAS NEVER BEEN TESTABLE OFF THE BUILD MACHINE — every serve
  target is plain HTTP.** `tools/cors-serve.py` binds `0.0.0.0` over HTTP, and browsers only
  permit service workers in a **secure context**, whose sole plain-HTTP exceptions are
  `localhost` / `127.0.0.1`. A phone or a second machine reaches a dev build by **LAN IP**,
  where **no worker registers, nothing is cached, and there is no offline path at all** — so
  the shell cache, offline boot, the build-scoped worker key and `dropSupersededBuilds` are
  all unreachable from any device but the one that ran `make`. This is why an operator's LAN
  experience and local test results have disagreed: *"I refreshed offline and it hung forever"*
  on a LAN address is not an SW bug, it is the **absence** of the SW, and the hang is the
  browser's own connect timeout against a host that left the network.
  **Fix: `make serve TLS=1`** (`tools/dev-cert.sh` + `--tls`), which mints a local CA and a leaf
  covering `localhost` **and this box's real LAN addresses**, detected under `--network host`.
  **A plain self-signed cert is NOT enough and this is the trap:** Chrome refuses SW
  registration on a cert-error origin **even after you click through the interstitial** —
  the page renders normally and registration fails with `SecurityError: … An SSL certificate
  error occurred when fetching the script`, so it looks like it worked. Firefox honours a
  manually-added exception, so a naive self-signed setup passes on one browser and fails on
  the other, which reads as a browser bug rather than as our misconfiguration. The device must
  **trust the CA** (`dev-cert.sh` prints the per-platform install); the Chrome-only escape is
  `chrome://flags/#unsafely-treat-insecure-origin-as-secure`, which needs the origin to match
  scheme, host **and** port exactly.
  *Diagnostic first, always:* devtools → Application → Service Workers. An empty list means
  every SW-shaped hypothesis about that origin is void before you start.
  [`DESIGN-CODE-AXIS-RECOVERY-AND-BOOT-SLOTS` §1.1a, G0; `REFERENCE-BOOT-AVAILABILITY-AND-RECOVERY` §4]
- **`podman stop -a` STOPS EVERY CONTAINER ON THE HOST, INCLUDING OTHER PEOPLE'S.** This box
  runs concurrent e2e grids, GUI sessions and playbook containers from other worktrees and
  other sessions. Many are started `--rm`, so stopping them **removes them permanently** —
  there is no restart, and anything mid-run dies mid-run. *Incident (2026-08-27):* `stop -a`
  run to clear a single port conflict took down ten containers, seven irrecoverably, and the
  conflict was already being avoided by the port change in the same command — it bought
  nothing. **Rule: never `-a` / `--all` against podman. Stop containers you named and started,
  by name. For a port conflict, change the port.** A busy port is not evidence that the thing
  holding it is yours.
- **A real two-browser WebRTC test is browser↔browser on a SHARED podman bridge.**
  Native has no `RTCDataChannel` (UDP hole-punch only), so both peers must be
  browsers; and rootless **pasta mirrors the host IP** into a default-network
  container, so a host-net + bridge pair advertises colliding host candidates and
  ICE fails — put both firefox containers on one `podman network create` bridge
  (distinct routable IPs) with `media.peerconnection.ice.obfuscate_host_addresses=false`
  (raw-IP host cands; mDNS `.local` won't resolve cross-container). Reproduction:
  `tools/e2e/webrtc-rung1/` + its ROUTING doc. **Rung 1 is GREEN** — the
  id-encoding asymmetry that doc names as the blocker closed 2026-08-05 (mutual
  minting, core-rust `0eccb3f`; `STATUS-2026-08-05-webrtc-rung1-bidirectional-green-mutual-minting`)
  and `make e2e-webrtc-chat` passes today: two browsers, chat over WebRTC. Read
  the ROUTING doc for the reproduction recipe, not for current status.
- **EVERY WEBRTC GATE TURNED OFF mDNS OBFUSCATION, SO THE MOST COMMON TOPOLOGY HAD NO COVERAGE —
  `make e2e-webrtc-lan` closes it.** Chrome and Firefox both ship `obfuscate_host_addresses` **on**:
  a host candidate's IP is replaced by a random `{uuid}.local` name resolved over multicast. All six
  spikes here set it **false**, on the assumption it would not resolve between podman containers — so
  every green gate ran raw-IP host candidates, **which is not what a user's browser sends**. Measured
  2026-08-19: multicast *does* cross a podman bridge, the accommodation was never needed for this
  topology, and two browsers on one LAN connect on `.local` candidates with **0 reflectors** — no
  STUN, no TURN. **The gate asserts the pref is IN EFFECT** (≥1 `.local` candidate both sides,
  candidates printed on pass) before crediting the connection, because Firefox scopes obfuscation by
  permission state: *"the pref was set and never bit"* is a real outcome, otherwise indistinguishable
  from success. `E2E_MDNS=1` is the knob; the NAT rigs keep raw IPs deliberately (`.local` cannot
  resolve between *isolated* networks, and `traverse` is about srflx), so it belongs to the topology
  that is genuinely a LAN, not to the spike. **The shape to carry: a pref set to make the rig work
  silently narrows what green claims, and nothing in the gate says so.** Full scenario matrix:
  `reviews/ANALYSIS-NAT-REACHABILITY-WHAT-WE-SUPPORT-AND-WHAT-WE-SAY.md`.
- **`make e2e-webrtc-nat` is the NEGATIVE control, and passing means the connection
  failed on purpose.** Two browsers on **isolated** podman networks (`--opt
  isolate=true`), node reachable only through the host — the shape that matters
  to the app: rendezvous path intact, direct path absent. It asserts rendezvous
  still works and media does **not** cross. **Scope, measured not assumed:** a
  host-run listener observes both containers arriving from the *host's LAN
  address*, so they sit behind ONE external address with per-flow ports — a fair
  model of "host candidates are useless here", **not** two independent NATs, and
  it does **not** predict whether STUN alone would fix it (that needs
  hairpinning). Don't cite it as evidence about STUN until that is tested. Measured contrast against the shared-bridge gates: meet-by-name ✅ both
  ways in both topologies; message delivered ✅ shared / ❌ split; OFFER deposits
  **4/side** shared vs **~970/side** split. The §11.5 single-flight bound is
  deliberately **not applied** in split mode — it is a property of a *completing*
  establishment, and ~970 is retry accumulating against one that never can; the
  script says so where it skips it and prints the number, because the rate itself
  is a question for a node operator. **The rig probes its own control** (A→B must
  be blocked in split, must be 200 on the shared bridge) — a leaked isolation
  would "prove" host candidates traverse NATs. When ICE lands, drop
  `EXPECT_NO_MEDIA=1` and the same rig becomes the positive traversal gate.
- **Before blaming the app for a browser `NetworkError`, prove the origin is yours.** The
  cross-origin naming e2e failed with a bare `NetworkError` that read exactly like a CORS bug in the
  app; the real cause was a **stale `python3 -m http.server` from another session squatting the
  port**, so `cors-serve.py` never bound (its stderr was sent to `/dev/null`) and the browser was
  talking to a server that serves 404s and no CORS. Two rules earned: **a rig's own server must be
  probed before the browser is asked to trust it** (`assert_federation_origin_healthy` checks
  200 **and** the `Access-Control-Allow-Origin` header, and names the squatter hypothesis in its
  failure text — mutation-checked by swapping in `http.server`, which reports `200=true cors=false`),
  and **never `Stdio::null()` a helper server's stderr** — a bind failure is precisely the error that
  makes the rig lie. Same lesson as the NAT rig's "prove the network path with a bare UDP punch",
  one layer up. Ports: pick a free one (`pick_free_port`) rather than a fixed favourite, but note the
  federation's origins are **baked in at publish time**, so the port must be chosen *before* the emit.
- **A budget for a MISS must be bigger than for a hit.** The e2e's unbound-name probe was given 2.5s
  because a successful resolve returns fast; a miss is slower — the walk still fetches the manifest,
  the signature and the interior nodes before concluding the key is absent. It presented as "the
  command produced no output", which reads like a broken verb rather than a short budget.
- **THE MULTI-HOST GATE: BOTH LEGS ARE CLOSED — topology by `make e2e-federation`, cross-impl by
  `make crossimpl-go`.** §1b wants a real publisher, a separate consumer peer, hash **and** signature
  verify at the consumer, and **the two on different hosts — not loopback, not same-process**,
  because `EXTENSION-SIGNALING` §11.5.1's blindness class is exactly *"every individual step reports
  success."* (Arch's own packet on it is **their** `ROUTING-2026-08-20-i` §4 — note the letter
  collides with ours; when a packet is cited here without a repo, check both trees.) **Three of the
  four legs already existed and were separately green** — `make federation` emits a real publish,
  `a_name_resolves_cross_origin_to_a_verified_page_in_a_browser` walks the whole chain in a real
  browser with real CORS, and `crossimpl_go.rs` consumes a real Go emission. What none of them varied
  is the **host**: both origins are `localhost:{port}`. What none of them varied *together* is the
  **implementation**: `e2e-federation` is our emitter at both ends. Both are closed now, below —
  what remains open on the row is the **resolve leg** (`name → binding → transports → fetch`), which
  no cross-impl emission carries yet.
  - **`federation_target()` makes the origin injectable** (`E2E_FED_ORIGIN` + `E2E_FED_REGISTRY`),
    and the walk is byte-identical in both modes so the *topology* is the variable, not the code
    path. It **refuses a loopback origin** — `E2E_FED_ORIGIN=http://localhost:…` would pass every
    assertion while proving what the local mode already proves, and a rig that can quietly degrade
    into the thing it replaces is the NAT rig's lesson one layer up. The registry peer-id must be
    passed in rather than read from the publisher's `MAPPING.txt`, or the shared filesystem is a
    same-process shortcut wearing a network.
  - **`make e2e-federation` IS THE GATE AND IT IS GREEN** — publisher, app origin and browser as
    three separate containers on one bridge, three addresses **asserted distinct**, the test process
    orchestrating from the host and serving nothing. A browser on one host resolves a name from a
    registry on another, walks both signed roots, and verifies the page bytes.
    `make federation-multihost` stands the rig up alone (`DOWN=1` tears it down); the gate is
    deliberately **not** part of `e2e-worker`, which would make the everyday suite need its own
    network and browser.
  - **ROOTLESS PODMAN DECIDED THE TOPOLOGY TWICE, AND BOTH TIMES THE SERVER WAS FINE.** (1) The host
    has **no route into a bridge network** — the first probe failed from the host with the server
    demonstrably running (`Address in use` inside the container), so the probe runs from a container
    on the network, which is the vantage the consumer has. That is also why the *test* skips the
    health probe in external mode: the orchestrator is not the consumer, and a probe from a vantage
    nobody under test uses is evidence about nothing. (2) The bridge **gateway is not the host** —
    it lives in the rootlesskit/pasta netns — so serving the app from the host and pointing the
    browser at the gateway gives `connectionFailure` against a live server. The app got its own
    container, which is the better shape anyway: the app origin becomes a genuine third party rather
    than the orchestrator wearing a hat. **Neither failure looked like a network-namespace problem;
    both looked like a broken server.**
  - **STDOUT OF THAT RIG IS AN ENVIRONMENT, SO EVERYTHING ELSE GOES TO STDERR** — the caller turns
    each line into a `-e` flag. `podman exec -d` prints its exec ID to stdout and shipped
    `-e <64-hex>` to the test container for one run. `log()` was already stderr-only *and the note
    saying why was already written*; the leak came from the one line that was not a log. There is a
    `grep -qvE '^[A-Z0-9_]+='` guard in the make target now, because that invariant with no
    enforcement point is exactly the theater this file warns about.
  - **What a green run there will and will not claim.** Will: separate network namespace, distinct
    routable address, real TCP hop, real cross-origin CORS in a real browser, verification at a
    consumer sharing no process and no filesystem with the publisher. **Will not:** two physical
    machines, the public internet, TLS, a CDN, or any NAT. Say both halves — a rig that overstates
    its scope is how a green gate launders an untested claim.
  - **THE CROSS-IMPL HALF IS GREEN — `make crossimpl-go`, core-go `dabd076`.** A browser-side reader
    enters at core-go's advertised manifest, verifies their two-hop signature against the key their
    peer-id carries, walks their CHAMP trie from the **signed** root, resolves all three authored
    keys byte-exact and enumerates the root to exactly that key set — over a real TCP hop, no shared
    process or filesystem. That is §1b's *"two implementations on one chain"* clause discharged, and
    it is the first time anything in this tree has walked a **live** foreign signed root (the
    workbench-go gate is a vendored directory; `e2e-federation` is our emitter both ends).
    `src/content_site/crossimpl_go_live.rs`; receipts `ROUTING-2026-08-20-h`, `-i`, `-j`.
    - **IT WAS RED TWICE FIRST, AND THE SECOND RED IS THE ONE TO REMEMBER.** (1) `cd8564a` signed a
      literal `root_hash` (`digest[i] = 0xC0 + i`) the origin 404'd → `IncompleteWalk`. (2) `ec30e96`
      served a **real, verifying root that committed to nothing** — an empty CHAMP node
      (`map: 00000000`, `data: []`), so every key was `Absent`; cause was
      `BuildTrieForPrefix(cs, li, …)` handed the **raw** index while entries were written through a
      `NamespacedIndex`, and that function **lists with the unqualified prefix while trimming with
      the qualified one**, so a raw index yields an empty trie *with no error*. **A structure that
      one side writes and another reads must be reached through ONE key space** — and a function that
      lists in one and trims in another is correct exactly when it is handed the right index, which
      is not a property it can state.
    - **THE RATCHET, AND IT IS THE ONE TO CARRY OFF THIS WHOLE ARC: a self-check must exercise the
      consumer's TERMINAL operation, not the last hop you added.** go's fix for (1) shipped a probe
      doing exactly what (1) taught — *"root fetched by hash → 200; old fake hash → 404 as
      control"* — and it **could not see (2)**, because fetching the root is not the consumer's
      operation either. The consumer's operation is **resolve a key**. A negative control on the hop
      you just fixed proves that hop and says nothing about the hop after it. Applies to us
      identically: any gate we write for a deliverable *someone else* consumes owes the last step,
      not the newest one.
    - **THE RATCHET, AND IT IS THE ONE TO CARRY OFF THIS WHOLE ARC: a self-check must exercise the
      consumer's TERMINAL operation, not the last hop you added.** go's fix for (1) shipped a probe
      doing exactly what (1) taught — *"root fetched by hash → 200; old fake hash → 404 as
      control"* — and it **cannot see (2)**, because fetching the root is not the consumer's
      operation either. The consumer's operation is **resolve a key**. A negative control on the hop
      you just fixed proves that hop and says nothing about the hop after it. Applies to us
      identically: any gate we write for a deliverable *someone else* consumes owes the last step,
      not the newest one.
    - **A CATCH-ALL ARM THAT NAMES ONE CAUSE IS A DIAGNOSIS WITH NO EVIDENCE — ours had one, and the
      state change is what exposed it.** Our signature gate gave `IncompleteWalk` its own arm and let
      everything else fall to `panic!("… did not verify against the key its peer-id carries")`. The
      moment go's root became real-but-empty the walk ended `Absent`, and our gate **blamed their
      signature for a tree that had verified perfectly**. It survived exactly until the state it
      never considered arrived. Match the variants you have reasoned about; make the residual arm say
      *"unclassified"*, never a cause.
    - **WHEN A WALK FAILS, ASK THE ROOT WHAT IT DOES HOLD.** The resolve gate now runs
      `SignedSession::enumerate` on failure and reports the committed key set — otherwise "short
      walk", "differently keyed" and "empty" all reach the reader as one word, `Absent`. The keys are
      *in* the nodes, so this is always answerable, and the enumeration rides the same pump and the
      same verification (not a back door around them). **Sixth appearance of the standing seam** —
      *"absent" and "withheld"/"empty"/"corrupt" keep arriving as the same value* — and the first
      time the answer was to make the failure **report its own context** rather than to give the
      outcome a new type.
    - **What DID cross, and it is worth having**: their manifest decodes in our decoder, their
      **two-hop** signature resolves, and it verifies against the key their peer-id carries — real
      TCP hop, no shared process or filesystem. Asserted *through* the failure, which is the tight
      part: `IncompleteWalk` is only reachable **after** `fetch_root()?` → `verify_signed_root`, so
      one assertion carries both facts and a wrong signature cannot satisfy it.
    - **Their leaf path is POINTER-trusted, not signed-root-walked** — `system/hash` pointers off the
      location index, hash-verified. Same sentence as our Site Browser: true, and the safety
      inference does not follow, because the origin supplied the pointer. Don't credit a green
      wire-face drive as a signed-root result.
    - **Our convention fallback is wrong about them in a way that matters**: they serve **flat**
      `content/{hex}`, we assume `sharded-2-4`, so `name pin <pid> <origin>` at a foreign origin 404s
      every blob *before* reaching the closure. §6.5.4 lets a consumer be *handed* a layout, and we
      have **no affordance to enter one** — that is ours, and it is why "their browser consumes
      unchanged" was never a one-line swap.
    - **`#[ignore]` WITH A REASON, not an env-gated early return.** A gate that reads its env and
      returns prints to stderr, which `cargo test` swallows on a pass — so three unrun gates read as
      three green ones in `make test`. Ignored-with-reason puts them in the count *and* prints why;
      the env gate stays as the second belt. Any on-demand gate added here owes the same shape.
- **Test the published chain over a SOCKET, not just off disk — and know exactly what that buys.**
  `four_domains_resolve_and_serve_over_real_http_with_cors` serves the emitted federation from a real
  `TcpListener` with the runbook's CORS + cache headers and walks both hops per name (measured: **4
  names, 35 requests, all 200**). The half that is *only* provable here is **CORS**: the harness
  treats a 200 with no `Access-Control-Allow-Origin` as a transport failure, because a browser does
  and `curl` does not (`a_200_without_cors_is_treated_as_a_failure_not_a_success`). **Do not claim
  more than that** — this seat wrote "catches a wrong URL that the disk tests cannot", then measured
  it: reintroducing the `published-root.bin` bug fails **both**. What differs is the *diagnosis*
  (a 404 naming the path asked for vs. a missing file). Related and load-bearing: that same URL fix
  is what makes `tools/cors-serve.py` mark the signed root `no-store`, since it keys mutability off a
  `published-root` suffix — the path bug and a cache-staleness bug were one bug.
- **A fixture that is hash-addressed is only deterministic if every input to the hash is.** A
  registry test used a real clock for `issued_at`; `issued_at` goes into every binding body, so every
  run produced different binding hashes → different trie keys → a different HAMT shape → a different
  set of names under any given link, and the test failed on the runs where the link it picked held
  none. Pin the clock (`ISSUED_AT_MS`), and pick a structural victim **by effect** (a link whose
  subtree actually holds what you are about to hide) rather than by position — under hash-keyed
  routing, "the first link" is not a stable referent.
- **THE UNFILTERED E2E WAS RED ON `dev` FOR TWO COMMITS AND NOBODY KNEW — a stale assertion, not a
  flake.** `97ca3435` narrowed the Meet mode picker to `tag` + `lobby` on purpose (`tag` and
  `secret` are the SAME derivation, so offering both is a footgun with a label — the entry above
  says so) and left `tests/e2e_worker.rs` asserting `"tag,secret,lobby"`. **The one place that
  catches a rule change is a gate that clicks the control, and this suite is expensive enough that
  it gets run last** — which is exactly how a deliberate narrowing and a red suite coexisted across
  two commits. Same shape as *"a rule that REMOVES a control breaks every test that clicked it"*,
  one turn on: here the rule removed an OPTION. Fixed, with the reason inline so nobody re-adds
  `secret` to make it green.
- **`wait_for_boot`'s FIXED `8000` WAS THE `ASYNC_ROUND_TRIP_BUDGET` LESSON, UNAPPLIED IN FIFTEEN
  PLACES — now `BOOT_BUDGET_MS`.** Three consecutive unfiltered runs failed on it at a **different
  phase each time** (21, then 20, then 19), always as `never saw 'Frame loop started'`, which reads
  as an app that will not boot at a phase with nothing to do with what changed. Measured across a
  healthy run, boot is **108–711 ms** — so 8 s was already an 11× margin and still failed, and that
  is the constant's own stated diagnosis: *a budget that generous which still fails isn't tight,
  it's load-sensitive, and a bigger guess doesn't fix that shape.* Raised to 30 s **with the
  measurement in the doc comment**, and — the half that is easy to skip — `wait_for_boot` now
  **says so on success** past `BOOT_SLOW_NOTICE_MS` (4 s), because raising a budget silently trades
  a false failure for a lost signal. Phases 19 and 20 went from failing to passing on the next run.
- **A FAILING e2e RUN LEAKS ITS SELENIUM SESSION, AND THE NEXT RUN INHERITS THE MESS — measured
  3-for-3.** When the suite panics mid-run it does not delete its session, so the one-slot grid
  stays `ready: false`. The next thing to touch it either queues for the 300 s session-timeout or is
  reaped by `setup()`, which is where `InvalidSessionId` and 120-second session-creation hangs come
  from — **including the ones that get blamed on another seat.** Two of the "the grid is busy, it
  must be somebody else" readings in this session were **my own previous failure's corpse**. Clear
  it before diagnosing anything: walk `/status` and `DELETE /session/{id}` (an API call, not a
  process kill — this box forbids those). Until the suite cleans up after itself, **a failed run
  must be followed by a grid clean, or the next run's failure is not evidence about the code.**
- **`wait_for_boot` NOW DUMPS THE PAGE LOG ON TIMEOUT, and that is what cracked Phase 20.** It used
  to fail with `never saw 'Frame loop started' in Nms` and nothing else — indistinguishable between
  a wedged worker, a multi-tab lock, an OPFS refusal and a panic during boot, every one of which has
  a distinct signature in `window.__entity_browser_log`. With the dump, one run showed boot reaching
  `worker spawn ok — queuing attachment` (`app.rs:753`) and **never** reaching
  `worker bootstrap: Ready handshake complete` — i.e. a second worker's Init never completed, with
  two `[entity-worker] calling wasm_bindgen` lines and only one `init resolved`. Same rule as the
  `Stdio::null()` one: **never discard the output that identifies the cause.** (What that race is
  remains open; Phase 20 passes intermittently, and it was not measurable while the sibling tree was
  moving.)
- **The two WebRTC gates are not interchangeable — run `e2e-webrtc-meet` before
  believing a connectivity change works.** `e2e-webrtc-chat` supplies the
  signaling node, the counterpart's peer-id **and** `?webrtc_enable=1` by URL —
  every one of which is something a user does not have and cannot type. It was
  green for a whole session while the shipped build installed **no establisher at
  all** (AP22). `e2e-webrtc-meet` hands the browsers nothing: they add the node
  through the Shell, reload, `meet tag <label>`, and chat over the id each one
  *learned*. When both are red, the pair tells you which layer moved.
- **Run `make e2e-worker` for any peer-routing / arm-dispatch / peer-display
  change** — worker peer routes register *asynchronously*, so a fresh peer can
  be invisible while compile + unit tests stay green. `--no-run` compile-check is
  not sufficient. The default browser arm is Worker-or-IDB, never Direct — a
  Direct-only test proves nothing about the shipped surface.
  - The **Tauri phases (14 / 15.6) are display-gated**: with a display on the
    host, the Makefile hands the e2e container the compositor socket
    (`E2E_DISPLAY_ARGS`, RUN_GUI-style) and they run for real; headless they
    **self-skip loudly** instead of failing the suite red (a permanent known-red
    masks new regressions). They are **port-independent** — the spawned listener
    falls back to a dynamic port when 4041 is taken (your own Tori can stay up)
    and the test reads `ws_addr` from the READY line. On timeout the phase
    reports the child's recent stdout + captures stderr to
    `target/e2e-tauri-stderr.log` (the GTK/display cause prints there).
    **14 and 15.6 are the only phases allowed inside that gate** — Phase 11 was
    found re-indented into it, silently unrun on every headless box; when you
    display-gate something, gate exactly the phase that needs the display. The
    **QR-hidden check and the measurement checkpoint were the next two found in
    there** and are now out (third session): the QR check asserts the QR panel is
    *absent*, so having no listener IS its fixture — it needed neither Tauri nor a
    display, and never ran headless. That makes three; when you touch this block,
    check what drifted in rather than assuming the list is current.
  - **Suite status as of 2026-08-13 (second session).** The **theme-dropdown
    (26.8) and site-editor-delete flakes are FIXED** — they were one bug, and it
    was not in either surface: the worker proxy's cache is *read* across every
    subscription's mirror but was *invalidated* per-subscription (see the
    cache-union gotcha below; `AUDIT-THEME-DELETE-STALE-DROPDOWN-2026-08-13`,
    CLOSED). A 10-run soak took the site-editor delete from a measured **29%**
    failure rate to **9/9 clean** with 26.8/26.9 8/8 beside it. The programs grid
    (2h.3) was fixed the session before.
    **The Tauri/display-gated phases are fixed too (third session), and an
    unfiltered run is now GREEN — 14/14, 285.93s.** The cause was **in the
    harness**: `start_tauri_listener` kept the child's stdout receiver in a local
    that dropped as soon as startup succeeded, closing the pipe — so for the rest
    of the suite every log line in the Tauri process hit `EPIPE` and dumped a
    3-line failure block to stderr, and the child's stdout was discarded, leaving
    a failing Tauri phase with no evidence. The reader thread now drains for the
    child's whole life into a ring buffer that the failure messages print
    (stderr: **32,618 bytes / 33 broken-pipe blocks → 218 / 0**). The fixed 3s
    budgets went with it — see `ASYNC_ROUND_TRIP_BUDGET` below.
    **The EGL/dri2 lines in `target/e2e-tauri-stderr.log` are NOT the flake** —
    they appear on fully green runs (software compositing is deliberate); the old
    guidance to read them as display contention is **withdrawn**. Check
    `podman ps` all the same — competing containers inflate the rate.
    (`STATUS-2026-08-13-tauri-phase-flakes-were-a-closed-stdout-pipe`.)
    **A wide `InvalidSessionId` failure is the session reaper, not your diff — see that entry in
    Build & test, and do not re-derive it from `podman ps`.** Confirmed independently by two seats
    within an hour on 2026-08-21: one measured the reaper, the other (this one) blamed an orphan
    `cors-serve` container and a restarted grid, cleaned both, and got a *different* wide failure on
    the next run. Removing orphan containers is worth doing and is **not** the cause; the cause is
    that `setup()` DELETEs every session on the grid, so two people running anything WebDriver at
    once take each other down. Re-run the failures in isolation before believing any of it.
    **The method that cracked it, when you meet the next one:** don't re-run and
    hope. Make the *test* deterministic (Phase 26.9 drives the delete from the
    tree so the optimistic local update can't mask the reconcile), print the
    trace on **pass** as well as fail so a green run still carries evidence, and
    **measure the base rate** before believing a few green runs.
  - **The full suite is ~4.5 min; narrow it while iterating.**
    `make e2e-worker T=<test-name-substring>` selects among the 13 tests;
    `UNTIL=<phase>` stops the 53-phase monolith once that phase is done;
    `SKIP_BUILD=1` reuses `dist/` (dev only — it has no `demo-apps`, so
    Phase 2h.2 fails for a fake reason). There is **no `FROM=`**: the monolith
    is one stateful chain, so jumping in would fail on absent prerequisites and
    read like a regression. `make e2e-phases` lists what both accept. **Run it
    unfiltered before landing.**
- **The e2e cannot hang silently — keep it that way.** Three guards, which a new phase
  inherits for free: the **stall watchdog** (`note_progress` rides `phase_gate!`; no
  progress for `E2E_STALL_SECS`, default 240s → prints the stuck phase and exits),
  explicit **WebDriver script/pageLoad timeouts** (30s/60s — the WebDriver default
  `pageLoad` is **300s**), and `timeout --signal=KILL $(E2E_TIMEOUT)` (15m) around
  `cargo test`. Don't add a wait none of these can see, and don't "fix" a watchdog firing
  by raising the budget before finding what actually wedged.
- **Wait on an async round-trip with `ASYNC_ROUND_TRIP_BUDGET`, not a fresh literal.**
  The e2e's fixed 3s deadlines were each justified by a guess about an unloaded box
  ("ws connect is fast on loopback — 2s is plenty"); measured, the healthy path is
  **15ms** — 200× margin, and it still failed under load. A budget that generous which
  still fails isn't tight, it's *load-sensitive*, and a bigger guess doesn't fix that
  shape. The constant is an upper bound (every loop returns on its condition, and the
  connect loops break out immediately on the explicit `✗ connect …` line, so a real
  failure is still fast) and sits well under the 240s stall watchdog. **Print the
  elapsed time on success** when you raise a budget — a tight budget accidentally
  signals "this got slower", and the print keeps that signal without the false failures.
- **Never `sleep(fixed)` then assert — poll with `poll_json`.** A fixed sleep encodes a
  guess about how long an async re-render takes; on a loaded box the guess is wrong and
  the phase fails for a reason unrelated to the behaviour under test (this shape caused
  the observed Phase 26.8 flakes). Polling also keeps a headless page honest — with no
  WebDriver interaction the rAF loop can go unpumped, and each `execute` forces a flush.
  Budgets are upper bounds, so be generous: a healthy run returns on the first poll.
  **~191 fixed sleeps remain** suite-wide — the known systemic source of load-dependent
  flake; converting one is a `poll_json` one-liner.
- **A new window/feature must extend `tests/e2e_worker.rs`** with a phase that clicks it and
  asserts on output, not just ride the boot-spawn loop — the loop only checks for panics, so a
  window that renders nothing at all rides it green. (**Correction, measured 2026-08-19:** the
  spawn list is *not* a hard-coded array — Phase 2 **discovers** the roster by reading
  `button.spawn-btn` out of the palette and asserts a floor, `MIN_DISCOVERED_WINDOW_TYPES`. So an
  addition is exercised automatically and a *removal* trips the floor deliberately. The old
  "hard-coded array silently skips additions" wording was wrong about the mechanism while right
  about the conclusion.)
- **THE SESSION REAPER DELETES *EVERY* SESSION ON THE GRID, NOT THE STALE ONES — so nothing else
  may touch Selenium while the suite runs, and the suite is hostile to anything that does.**
  `setup()` walks `/status` and issues `DELETE /session/{id}` for **every** id it finds; "stale" is
  an assumption about who else is using the grid, not a property it checks. Measured 2026-08-21:
  driving ~25 of my own WebDriver sessions against `:4444` (a manual book-anchor investigation) put
  the suite into `13/19`, with failures reading `Unable to find session … reason: session closed
  normally (QUIT command)` — i.e. **the harness quitting a live session and then failing the test
  that owned it**. Two consequences: never run a WebDriver probe beside the suite, and treat a
  failure whose text is `InvalidSessionId` / `Reached error page` / `Unable to find session` as
  **infrastructure, not a defect** — no assertion ran.
- **THE UNFILTERED SUITE IS FLAKY ON A LOADED BOX, AND THE FAILING SET IS DIFFERENT EVERY RUN —
  measured, and it supersedes the "unfiltered run is GREEN 14/14" note above.** Four consecutive
  full runs on 2026-08-21, same commit: **13/19 · build-failed · 16/19 · 15/19**, in 322s–651s
  (2× spread), with a *different* set failing each time — `worker_boots_and_opens_all_windows`,
  `selecting_a_connector_says_it_needs_a_reload`, `default_idb_boots_into_remote_deployment_home`,
  `frontend_idb_peer_lifecycle`, … **Every single one of them passes in isolation** (`T=<name>`,
  2.4s–145s, all green) — which is what said the failures were in the harness, not the code.
  **FOUND AND FIXED. After the two fixes below: 19/19 three times running, 351.44s / 351.29s /
  351.26s** — and note the *variance* went with it (322–651s before, 0.2s spread after), which is
  the stronger signal that the cause was queueing rather than load.
  - **THE MECHANISM, because "contention" is not a diagnosis.** The Selenium standalone image
    serves **one session at a time** and *queues* further requests for its `--session-timeout`
    (**300 s**) — the reaper's own doc comment says so. `reap_stale_sessions()` DELETEs the
    lingering session and **returns immediately**, but a DELETE returns before the node frees the
    slot, so the very next `connect()` can land while the grid is still busy and **queue** instead
    of failing fast. That is where `InvalidSessionId: Tried to run command without establishing a
    connection`, `Unable to find session … (QUIT command)` and a **651 s run** (vs a healthy 322 s)
    all come from, and not one of those errors mentions the queue or the reap. The reaper now waits
    for the grid to report itself empty and warns loudly if it never does.
  - **Two harness defects found while chasing it, both breaking rules already in this file.**
    `start_dist_server()` sent the helper server's stderr to `Stdio::null()` — the exact thing the
    federation-origin entry forbids ("a bind failure is precisely the error that makes the rig
    lie") — and `setup()` then **slept a fixed 300 ms** and navigated. With no server there the
    browser reported `connectionFailure` at `localhost:8092`, which reads as a network or app
    fault. Both fixed: the port is **polled**, and an immediate exit is reported *with the child's
    stderr*. **Measured before blaming the sleep, and the tempting story was wrong:** bind latency
    here is **22–32 ms idle and 21–25 ms under 8-way CPU load**, so 300 ms was a 10× margin and did
    NOT cause these failures. A fixed wait is still wrong — it cannot tell "not yet" from "never" —
    but say what it did and did not do. Two further guesses died the same way: all 19 tests bind
    the handle as `let (client, _server)` (none drops the server early), and the tree was intact
    across the build failure.
    - **The readiness probe checks "is OUR CHILD ALIVE" BEFORE "is something listening", and that
      order is load-bearing — found by mutation, in the fix itself.** Holding :8092 with a socket
      that accepts and never answers, the first version saw the *blocker* listening, returned `Ok`
      over an already-dead python, and the failure surfaced 60s later as a WebDriver navigation
      timeout. **The cheap check shadowed the real one, inside a fix whose entire purpose was a
      missing diagnostic** — the same lesson Phase 19-doc taught twice, met a third time by the
      person who had just written it down. The probe is now an HTTP round-trip requiring **200 for
      `/index.html`** (a bare TCP connect proves only that *somebody* holds the port), and the
      child-exited branch is checked first. Mutation-checked both ways: port held → fails in
      **0.56s** quoting python's `Address already in use`; port free → same test passes in 10.13s.
      Before, that class cost 60s and reported as `connectionFailure`, i.e. as an app fault.
  - **One of the four runs failed at BUILD, not in a browser, and THE CAUSE IS STILL UNKNOWN —
    handed to the next session.** `error: manifest path
    /src/entity-systems/entity-browser-rust-apps/Cargo.toml does not exist`, exit 1, against a tree
    that was perfectly intact before and after (`Cargo.toml` present, `git status` clean but for
    two edits of mine). What the log actually shows, which is the part worth carrying:
    **trunk's FIRST cargo build SUCCEEDED** — our own 15 warnings, `bin "entity-browser"`,
    `Finished dev profile in 5.33s` — **and a SECOND cargo invocation then reported the manifest
    missing**, seconds later, over the same bind mount. So it is not a missing file and not a
    broken mount; something about the second invocation's manifest path resolution differs.
    **An earlier version of this entry asserted a `:z` SELinux relabel race with a concurrently
    running `site-serve` container. That was a guess, it is unverified, and it does not survive the
    log** — a relabel race would not let the first build through. Recorded as *seen once, not
    reproduced*: do not repeat the `:z` story as fact, and if you meet it again capture the full
    trunk invocation and whether a second container held the mount.
- **THE E2E SHELLS OUT TO `cargo run` AGAINST THE LIVE TREE — do not edit source while it runs.**
  Phase 21 emits its cross-peer fixture by invoking the binary, so an in-progress edit turns into
  a **compile error reported as a phase failure**, at a phase with nothing to do with what you
  changed (`fixture emit failed: … error[E0063]: missing field …`). It cost a whole run here. Two
  habits: start a run only from a tree that builds, and when a phase says *"fixture emit failed"*
  read the stderr it prints — that is a **compiler** message, not a test one.
- **A NEW PHASE MUST BE ADDED TO `PHASE_ORDER`, and only a FILTERED run tells you.** `phase_gate!`
  is a no-op when `UNTIL=` is unset, so a phase missing from that list rides an unfiltered suite
  perfectly green (mine did — 18/18) and then panics *"label is missing from PHASE_ORDER"* the
  first time anyone narrows the run. It also means `make e2e-phases` was silently under-reporting
  the suite. So: after adding a phase, run it **once with `UNTIL=<your label>`** as well. The
  general shape is worth keeping — *the unfiltered run is not a superset of the filtered one*, and
  here the cheap run is the strict one.
- **Assert on ELEMENTS, not on a name two surfaces both legitimately contain.** Phase 2h.2s
  checked "the launcher is gone" as *does the section text still say 'War'* — and failed on its
  first run because the Saves panel it had just opened correctly lists a save **named after the
  app that wrote it**. The string was true of both states, so the assertion could not tell them
  apart; `querySelectorAll('button.app-card').length === 0` can. A text search over a whole
  section is a substring match pretending to be a structural claim.
- **A NETWORK THAT *ACCEPTS AND NEVER ANSWERS* IS A DIFFERENT BUG FROM ONE THAT REFUSES, AND
  `python3 -m http.server` CANNOT PRODUCE IT.** A refused connection (interface down, DNS
  failure) rejects the promise promptly: every `.catch` and every `.ok()?` on the path is
  reached and boot continues — which is why "offline reload sometimes works" and why three
  instances of this bug accumulated as *code-reads nobody could reproduce*. An origin that
  completes the TCP handshake and then sends nothing never rejects anything, so an unbounded
  `await` on it does not fail — it does not return, for 75–130 s on Linux and effectively
  forever behind a captive portal. Before the frame loop starts that is a blank page **with no
  frozen-frame watchdog**, because the watchdog installs after `boot_load` returns.
  **`tools/e2e/blackhole-serve.py`** is the only thing in this rig that can produce it: it
  serves `dist/` normally and, for a nominated set of paths, reads the request and then writes
  nothing and closes nothing. The stall set is settable at runtime
  (`GET /__blackhole?stall=/a,/b`) because the service-worker half of the gate **must** cache
  the shell before the origin is black-holed — start it stalled and there is nothing to fall
  back to, so a pass would prove only that the app can fail. Gates:
  `boot_survives_a_blackholed_deployment_config` and `a_cached_shell_survives_a_blackholed_origin`.
- **BOTH BLACK-HOLE GATES ARE ANTI-VACUITY GUARDED ON THE SERVER'S OWN LOG, AND THEY HAVE TO
  BE.** A boot that never requested the black-holed path satisfies every assertion in the test
  for entirely the wrong reason, and the "fix" that would produce that is *removing the
  fetch*. So the server's stderr is drained and checked: the request must have **arrived** and
  been **stalled**. Measured at the wire, not reported by the thing under test — the same rule
  that made `DistServer::request_count` exist, for the same reason (Firefox zeroes
  `transferSize` for anything a service worker supplied, so the obvious in-page assertion is
  vacuous).
- **PRINT THE MARGIN, AND HERE IT IS ALSO THE VACUITY CHECK.** `boot_survives_a_blackholed_
  deployment_config` prints its boot time because the expected value is *the deadline plus a
  healthy boot* (~3 s + 108–711 ms). A time far **below** that means the stall was never
  reached and the green is empty; far **above** means something else on the boot path is also
  waiting and the §4A enumeration of "exactly two network awaits" is incomplete. A budget that
  only speaks when it fails cannot tell you either of those.
- **A GREP GATE IS VERIFIED BY MUTATION, NOT BY BEING GREEN — and check it against its own
  documentation first.** `tools/net-lint.sh` initially reported a violation in the very file
  that had just been fixed, because the fix's doc comment names the raw call it replaced; the
  cheapest way to green it would have been deleting the explanation (AP29). It now drops
  whole-line comments before counting — deliberately **not** a strip-from-`//`-to-end rule,
  which would truncate any code line holding an `https://` literal and turn a false positive
  into a false negative. Then reintroduce the violation and watch it fire. A lint that has
  only ever been observed passing has not been shown to do anything.
- **A GATE MUST BE RUN FROM A CLEAN INVOCATION OF THE TARGET THAT OWNS IT.**
  `the_app_reports_the_build_it_is_running` asserts the app's logged build id against the
  `entity-build` stamp in `dist/index.html`. `make wasm` and `make wasm-release` end in
  `./tools/build-stamp.sh`; the `make e2e-worker` build line did **not**, so the gate was
  green only for whoever had run `make wasm` first, and a clean `make e2e-worker` red it with
  *"tools/build-stamp.sh did not run"* — after it had been reported green (AP31). Fixed by
  putting the stamp in the e2e build line too. **If a gate needs a build step, that step
  belongs in every build path that feeds it.** This is the general form of the `SKIP_BUILD=1`
  warning: that names one way to inherit a stale `dist/`, this is the same failure arriving
  through a target that never built the artifact at all.
- **RE-RUNNING A PUBLISH FIXTURE DOES NOT REWRITE `entity-deployment.json`.** A publish whose
  content is already present in the tree takes the engine's idempotent path, and the emitted
  deployment document keeps naming whatever the *previous* publish named. A test that
  re-published publisher A to "correct" a document pointing at B got a document still pointing
  at B, and failed on a fixture artifact wearing the costume of a product bug. **To change
  what the domain declares, write the document** — `std::fs::write` the bytes captured
  earlier. It is also the more faithful reproduction whenever the scenario is about a
  *document* being wrong rather than about a republish.
  (`a_supersession_the_domain_contradicts_is_dropped`.)
- **DO NOT SEED `blackhole-serve.py`'s STALL SET WITH `/index.html`.** The server's own
  readiness probe fetches that path, so stalling it at startup deadlocks start-up and the test
  fails before the browser is ever involved. The harness names this ("either `dist/` has no
  index.html, or something else is holding the port — check before reading this as an app
  fault"), which is the only reason it costs a minute instead of an hour. **Start clean and
  call `set_blackhole(...)` at runtime** — the same reason the stall set is runtime-settable
  for the service-worker variant, where the shell must be cached before the origin goes dark.

## The recovery console (L1 BIOS)

- **THE BIOS IS THE STRICTEST CASE OF D23, NOT AN EXCEPTION TO IT.** `index.html`'s System
  Recovery console is what a user opens *because* something already hung. Every network read in
  it is bounded (`fetchWithDeadline`, 3 s) and expiry is a **reported state** — "could not
  reach the origin" — never a spinner. `tools/net-lint.sh` counts this file, and
  `the_recovery_console_survives_a_blackholed_origin` is the behavioural half. Neutering the
  deadline reds it with the panel stuck on "probing…", which is what the bug looks like.
- **A VALUE THE CONSOLE COMPARES MUST DECLARE WHERE IT CAME FROM (AP32).** "What does the
  origin serve now" is fetched over `/index.html`, which `sw.js` routes through `networkFirst`
  — so on a slow origin the answer arrives *from the cache the panel is auditing*. Reporting
  agreement there would tell a stuck user they are current. The panel reports **inconclusive**
  whenever a service worker controls the page, and reserves "current" for the uncontrolled
  case. The unspoofable signal is `registration.waiting`: it needs no network and it is the
  observed form of "an update is downloaded and blocked".
- **The negative result is half the point.** "You are running the build the origin currently
  serves" is what stops someone clearing site data on a wrong theory — the destructive E5
  action the brick matrix exists to prevent. Don't optimize it away as uninteresting output.
- **THE ROUTING MIRROR IS THE ONLY CHANNEL ACROSS THE L1/L5 BOUNDARY — AND IT IS A CONTRACT
  BETWEEN TWO FILES NO COMPILER CHECKS.** `boot_diagnostics::write_routing_mirror` writes
  `entity_routing_mirror` to localStorage on every boot; the BIOS `JSON.parse`s it. The
  authoritative value is a CBOR entity in the durable tree, which the console deliberately
  cannot decode because it must work when the peer does not boot. **Rename the key or change a
  field and you must grep `src/boot_diagnostics.rs` and `index.html` together** — the unit test
  `the_mirror_carries_the_routing_facts_and_when_they_were_true` pins the exact document and is
  the closest thing to a type on this seam.
- **STALENESS IS THE FEATURE, SO NEVER RENDER THE MIRROR AS "WHAT THIS PROFILE BELIEVES".**
  It is what the profile believed at `written_at`, running `build`. When the app cannot boot
  that old value is the interesting fact — but a reader who cannot tell "current" from "last
  known" draws a confident wrong conclusion from it, which is the exact failure the console
  exists to remove. The card always shows the timestamp and the writing build beside the value.
- **It is a mirror, not a second source of truth (AP17/AP30).** Nothing branches on it — no
  code reads it to make a decision, it exists only to be reported — and it is rewritten from
  the tree on every boot, so the re-derivation path AP30 demands is "boot again". Keep both
  properties if you extend it. Anything added here is rendered verbatim with no redaction, so
  it must stay public routing information: peer ids, site ids, timestamps. **No secrets, ever.**

## Localization, copy & the operator surface

- **"CONNECTOR" IS THE CODE'S WORD AND NOWHERE ON THE SCREEN — the surface says "Rendezvous
  nodes".** Reported by the operator as meaning nothing, correctly: it named neither end of what it
  describes. The chosen name is the word **System Overview already used** for the switch on the
  other side of the same relationship (a desktop *offers* rendezvous, a browser *picks* one), so
  the two windows now say one thing — and every locale already had a translation for it, which is
  what made the rename cheap. The module, the shell verb, the tree path, the entity type and the
  i18n **keys** keep `connector`; renaming those is a data migration for no gain. Also renamed:
  *Connect to a device* → **Connect by address** (the card dials one; Known devices reconnects and
  Meet finds an address for you — three surfaces that all read as "connect" until one said how).
  **Meet now renders ABOVE the node list**, asked for twice: a rendezvous node is plumbing you
  configure once, meeting somebody is the thing you came to do.
- **`tag` AND `secret` ARE THE SAME DERIVATION, SO THE PICKER OFFERS ONE OF THEM.** The operator
  read *"A word we agreed on"* / *"A private phrase"* and said they are the same thing; they are.
  `key::tag_key`/`secret_key` are both `derive(mode, input)` — SHA-256 over a domain string, the
  mode tag, and the bytes you typed — so **neither word ever leaves the device** and the node sees
  33 opaque bytes either way. The only difference is which domain the hash lands in, which is
  invisible to the person choosing and produces a **silent never-meet**: two people typing the same
  word under different modes derive different keys and are told *"nobody else was there"*. So the
  picker offers `tag` and `lobby`; the Shell keeps `meet secret <phrase>` for a client that uses it,
  and `meet_mode_secret` stays as the **label** a shell-started meet renders. The note that claimed
  a secret is *"not shown to the rendezvous"* is **deleted, not reworded** — it was drawing a
  privacy distinction between two things that hash identically, and the honest thing to say about a
  name is about its **entropy**, which is the only thing that actually varies. Generally: **before
  exposing two protocol modes as a choice, check whether the difference is observable to the person
  making it** — if it is not, it is a footgun with a label.
- **SYSTEM OVERVIEW'S CARRY-STRINGS LIVE UNDER THE SWITCH THAT PRODUCES THEM — the third card is
  retired and `sysoverview.card_connect` with it.** The previous pass split one pile into *This
  device* / *What other devices can do here* / *Connect another device* and wrote "do not fold them
  back together". The operator folded them on sight and was right: turning a switch on and finding
  out what it produced were two reading tasks in two boxes, joined by nothing. Now one *Other
  devices* card of `components::service_row`s — name + state chip + control + a sentence — with the
  served URL under the app-server row, the LAN pairing line under the rendezvous, and the internet
  one under port forwarding. Four things:
  - **`connect_steps` still owns the composition and its tests still guard it**; the renderer only
    *distributes* the result. What changed is that its "the URL leads" decision is now expressed as
    the ROW order, which is why the app-server row moved to the top and the unprovisioned state's
    pointer flips from *turn on Rendezvous above* to *below*.
  - **No carry-line can appear under a row that is off**, and that is inherited rather than
    re-asserted: `pairing_commands` refuses a line for a node that is off or not listening, and
    emits the internet one only when `external_addr` is `Some` — which is exactly *a door is open*
    and never *asked* or *refused*.
  - **`ServiceState` is the THIRD §S4 vocabulary** (`On` / `Incomplete` / `Pending` / `Refused` /
    `Off`), beside `ConnState` and `AuthState`, and it is a genuinely different axis: those describe
    a peer, this describes a switch on this machine. Five states because the port-mapping row's
    *"four different `None`s must not read as one"* rule is not row-specific — it is what the
    vocabulary is for, and collapsing it rebuilds the one-sentence failure on a new surface.
  - **NO GATE READS THIS WINDOW.** It is Tauri-only, the Tauri e2e phases are display-gated, and
    none of them opens System Overview — the hole the design doc already names (*nothing loads the
    app the way a user does*). The layout here is **unverified by any automated check**; the
    strings and the composition are native-tested, the rendering is not.
- **CHANGING AN EN STRING SILENTLY LEAVES 30 TRANSLATIONS ASSERTING THE OLD THING, AND
  `i18n-locale-check` PASSES.** The checker verifies **parity, slots, plural categories and
  script** — every structural property except whether the translation still means what the
  base says. So correcting the insecure-origin banner in `src/i18n.rs` left all 30 overlays
  in `locales/` telling non-English users *"the browser blocks peer-to-peer connections"*,
  which had just been measured false, and every gate stayed green. **Deleting the key is not
  an escape**: parity is enforced (a missing key is an error unless the locale is in
  `PARTIAL_OK`), so a semantic fix to a translated string means **updating all 30 in the same
  commit**. Cheap in one pass with a script that rewrites the single key and preserves
  formatting (`json.dump(..., ensure_ascii=False, indent=2)` reproduced the files exactly —
  30 files, 1 line each, no reformatting noise). **The general shape is this repo's most
  repeated one, in a new place: a check that validates STRUCTURE reads as a check that
  validates CONTENT, and the gap is invisible while every string happens to be true.**
  - **AND THE SAME GATE WAS HIDING A BIGGER ONE: 51 KEYS HAD NEVER BEEN TRANSLATED AT ALL —
    FOUND AND FIXED 2026-08-23, AND THE GATE IS NOW AN ALLOWLIST AT ZERO.** The entry above is
    about strings going *stale*; this is about strings never translated at all, and the same green
    line covered both. Measured: **51 keys verbatim English in all 13 non-Latin-script locales**
    (plus 4 more in one or two locales each) — the whole Registry Browser, all of Chat's
    empty/prompt/placeholder text, the site-directory verification sublines, the app-host failure
    messages — while `make lint` printed *"30 locales × 701 keys … parity, slots, plural
    categories, script all clean"*. Seven things:
    - **THE "script" IN THAT LINE IS A HOMOGLYPH RULE AND RUNS THE OTHER WAY** — *no Cyrillic or
      Greek letters inside a **Latin** locale*. It is exactly the check whose NAME suggests it
      would have caught this and whose direction guarantees it could not. **Read what a gate
      asserts, not what its summary line is called**; this is the `cfg`-line and constant-name
      lesson arriving in a success message.
    - **THE DEBT WAS 30 LOCALES WIDE, NOT 13, AND THE FIRST WRITE-UP SAID 13.** The gate can only
      *prove* the non-Latin ones, so the finding was framed in the set it could measure — but `de`,
      `fr`, `es`, `nl` … carried the identical English strings. **A finding scoped to what the
      instrument can see is a claim about the instrument**; check the unmeasurable half by hand
      before stating the size. All 30 are translated now (1,470 values re-checked against the EN
      base; the 17 that still match are real cognates — German/Dutch/Italian *Chat*, French
      *Source* / *Message…*, *byte*/*bytes*, and the pre-existing *Shell*/*worker*/*tick*).
    - **The enforcement point is `tools/i18n_untranslated_check.py`** (in `make lint`). Rule: a
      value in a non-Latin-script locale must carry a character of that script. It **prints the
      offending keys** — a count with no key list is a number nobody can act on.
    - **IT IS AN EXPLICIT KEY ALLOWLIST AT TARGET 0, NOT A BASELINE COUNT — and the baseline file
      is DELETED.** It shipped as a ratchet because the backlog was 668 and stopping the growth was
      the only move available. Once the backlog was translated, 78 anonymous survivors is a number
      nobody can review, so each of the 6 exemptions now carries its reason in `ALLOWLIST` (three
      palette proper nouns, OPFS, IndexedDB, and `siteeditor.markdown_label` — a format name plus a
      slot, with no prose in it). **The count could not have caught either of the two things this
      does**: a new untranslated string fails immediately with nowhere to absorb it, and a *stale*
      exemption fails too — an allowlisted key that is translated everywhere is reported as unused
      and must be deleted, so the list cannot quietly rot back into a baseline. Mutation-checked
      in both directions. **General rule: a ratchet is for a backlog you are paying down; when it
      reaches the floor, convert it to a reviewable list or it becomes a permanent tolerance.**
    - **IT CANNOT CHECK THE 17 LATIN-SCRIPT LOCALES, AND SAYS SO IN ITS OWN SUCCESS LINE.**
      "Status" is legitimately "Status" in Dutch; no mechanical signal separates a cognate from a
      skipped string. Those locales are **unaudited by machine** — only a human who reads the
      language can grade them. Saying so in the pass message is deliberate: a gate that overstates
      its scope is how the *next* one of these hides.
    - **The recent features were NOT the cause, which was checked before assuming.** All 18 keys
      added since 2026-08-22 are in all 30 locales with no English copies, and `i18n-lint`'s `raw`
      metric is **opt-out**, so newly hardcoded prose fails the build. The debt predates them: it
      is two windows that shipped *after* the localization pass and never got one of their own.
      **When a gate turns out to be blind, date the debt before blaming the newest commit.**
    - **`git checkout -- locales/` ATE AN UNCOMMITTED TRANSLATION ROUND, mid-session.** It was used
      to undo a one-key mutation test and it reverted **every** uncommitted file in that directory.
      The rule this file already carries about `git checkout <path>` being destructive to a
      *concurrent seat* applies just as well to your own unstaged work: **commit before mutation-
      checking, and undo a mutation from a copy you made, not from the index.**
- **THE OPERATOR SURFACE IS A SURFACE, AND NO GATE READ IT [AP25].** Everything the naming arc
  ships is exercised through Rust APIs; nothing typed what the CLI *prints*. Consequences found in
  one audit pass (`AUDIT-NAMING-AND-PUBLISHING-ARC-2026-08-18`): the help taught
  `--bind NAME=…` while the parser matched only `--bind=`, so the flag was dropped and the refusal
  said *"at least one --bind is required"* **to someone who had just passed one**; the help also
  omitted the `@ORIGIN` arch D10 makes mandatory, so following it could not succeed at all. The
  fix is not a better string — it is **`parse_registry_args` returning its message as a value**,
  because an exit-code assertion cannot tell "refused correctly" from "refused for the wrong
  reason" (both paths exit 1, which is why this survived). Any new CLI refusal owes a test on the
  *message*. Related and the same shape: **`make registry` exists now** (F3 — publishing a registry
  had no containerized route at all, only a bare `cargo run` a podman-only host does not have), and
  **`registry --verify` is a distinct verb from `publish --verify`** because the two resolve
  *different* durable identities (`publish/keypair` vs `registry/keypair`) — pointing `publish`
  at a registry directory looks under the wrong peer-id and calls a clean tree unverifiable.
  **Whoever owns the identity owns the verification.**


# entity-browser-rust — status

_Updated: 2026-09-20 · version **0.9.0** in the manifests, with a large body of post-0.9.0
work on the development branch. Two arcs: **deployment recovery** (a bad build has somewhere
to fall back to, a stuck visitor has a way out, a returning reader keeps their place), and
then **writing to the tree rather than only reading from it** — a feed you can follow, read
and post to, and three new windows (Feed, Files, System Monitor) that make the device you are
on legible. The gate table below was re-run in one pass at that tip — **every row, including
the end-to-end suite and the connectivity gates**._

_The next release is **0.10.0**, not 0.9.1: `publish` now refuses a silent site arm, which is
a breaking change to the publisher command line. Everything else measured on the contract
surface is additive — no flag, make target, top-level verb, deployment-config field, entity
type or window was removed._

_A build is identified by a **pair** — this application's commit and the `entity-core-rust`
commit it was linked against — because the two are joined by path dependency with no
cross-repo lockfile. Both halves are stamped into the shipped `index.html`
(`entity-build`, `entity-build-id`, `entity-core-ref`), so the question is answerable from
the artifact rather than from anyone's memory. `CORE_RUST_REF=<ref>` builds a named kernel
commit rather than whatever is on disk._

Where the product is, what is proven and on what, and what is open. It cites files, symbols and
measurements rather than commit SHAs, which do not resolve for a reader outside this tree (see
*Commit pins* below).

What changed for a user is in `CHANGELOG.md`.

Spot-check a claim before acting on it. This file has carried stale rows, and they survived by
being read instead of run.

## Where it is

`entity-browser-rust` is the DOM-primary Rust/WASM **reference application** on the entity-core
substrate: a window-manager / UI shell over the entity tree, rendered as HTML DOM in the browser
(WASM peer) and in a Tauri desktop WebView. It is a *binding / app* built on the Rust reference
implementation and its SDK — one worked example of the paradigm, not a mandate.

Maturity is **research preview**: suitable for evaluation and exploration, not a hardened
production deployment. HTML DOM is the only render path; the legacy native/egui renderer is gone
and `make native` prints a deprecation redirect.

**What a person can do in it today**, which is wider than it was at 0.9.0: browse and read
published content sites; run embedded HTML/JS apps, including virtual machines, and move files
between them and the device; connect to another peer, chat, and transfer files; publish a site, a
set of apps and a feed under one signed root; and — new since 0.9.0 — **follow publishers, read
their feeds with every post verified against its author's own signature, and post to a feed of
their own**, where the write into their tree *is* the publish. Three windows were added with that
arc: **Feed**, **Files** (what is on this device, and which places other devices can see) and
**System Monitor** (what this browser tab is spending its time on, and what the browser will not
tell it). None was removed.

## Gate state

Re-measured **2026-09-20** rather than quoted — **every row re-run at one development tip,
from a clean tree on both halves of the build pair**, including the end-to-end suite and the
connectivity gates:

| Gate | Result |
|---|---|
| `make test` | **2225 / 0 / 26-ignored** across **21** test binaries (main binary 2106). It has moved 1300 → 1484 → 1501 → 1575 → 2225. Re-run it; do not quote this line |
| `make test-tauri` | **59 / 0** across 4 binaries. (`src-tauri` is workspace-excluded, so it is not in the number above.) **Run it serially** — it embeds `dist/`, so any concurrent build that rewrites that directory fails it on a stale asset name |
| `make lint` | **exit 0 — seventeen checks**, not the seven this row carried through 0.9.0. `ui-lint` atoms=6 styles=133 hex=4 across 23 files · `net-lint`, `foreign-cache-lint`, `ecf-lint`, `fidelity-lint`, `vocab-lint` all match their baselines · `cache-policy-lint` 36 shared vectors (9 immutable / 27 mutable), which runs the doc check · `publish-doc-check` 27 flags · `webrtc-slot-check` · `i18n-lint` **raw=0** phys=0 · `i18n-locale-check` 30 locales × **1002** keys · `i18n-callsite-check` **893** call sites · `i18n-untranslated` 7 allowlisted · **`i18n-drift` 1002 keys with zero `stale` rows** — every overlay swept against the English it was translated from · `tree-hygiene` no tracked path is gitignored. Note `cargo clippy --features e2e --tests` is the only thing in the tree that compiles the end-to-end suite at all |
| `make e2e-worker` | **88 passed / 2 failed, 884.38 s** — unfiltered. **Both failures are filed, open and bounded; neither is a regression** (see below). **The invocation is part of the number:** `env -u WAYLAND_DISPLAY -u DISPLAY make e2e-worker WEBDRIVER=http://localhost:<free> E2E_HTTP_PORT=<free>`, on a **private** grid — this box routinely carries other seats' Selenium on `:4444` and a shared `:8092`, and a collision there answers with someone else's build rather than refusing. With a display attached the Tauri WebView phase is a standing red on this host |
| `make wasm` | exit 0 — build id `f81e65c5a029b555` |
| `make site-dist` | green — `check-dist` consistent, and `publish --verify` on the emitted tree reports **13 pointers / 13 verified / 0 broken; 15 blobs / 0 orphaned**, signed root verifies against the publisher key. Release build id `91aca59a6a39aa3f`; the closing summary prints the pair it was made of |
| `e2e-webrtc-{chat,meet,vanish,file}` | **4 / 4 PASS**, and **§11.5 deposits at 4/side on every one** — inside the O(1) bound of 8, and the figure the 2026-09-09 audit recorded as RED at 16/8/12/8. One run each; this metric is variable, so that is evidence it is not over, not proof it is fixed |
| `make fleet-probe` × 6 domains | exit **0** — fleet uniform, no mutable URL cached beyond correction anywhere |

**The two end-to-end failures, so a reader does not mistake them for new.** Both are recorded in
this repo's agent guidance with their bounds. `an_app_republished_…_on_the_worker_arm` is a
returning profile keeping an old app after the publisher republished — **intermittent at ~44% with
clustered outcomes**, and reachable only under `?worker=1`, which is opt-in; the shipped Direct
arm's twin stayed green. `a_drag_on_a_windows_grip_survives_the_window_rebuilding_under_it` is
intermittent with **the cause still open**, and the assertion that fires is not the one that
matters — it reports a rebuild mid-drag when the measured run shows none, while the drag itself
silently failed to land. A green unfiltered run on this tree is a run that won two coin flips; it
is not evidence either defect is gone.

**`i18n-lint raw=0` — the baseline file is empty, which is the floor.** It read `raw=90` earlier on
2026-09-03 (`doctor.rs` 66 + `content_site/mod.rs` 24) and both halves are closed, differently and
on purpose: the health checks were **translated into all 30 locales**, and the demo site's manifest
title, nav labels and page titles moved into `demo_content.rs`, which carries a file-level
`i18n-ignore-file` because **a published site's content is the publisher's words, not the app's
chrome** — the app does not translate the pages it renders. The axis is *who wrote the string*, not
*who paints it*; that distinction is written up in that file's header, where it had previously been
drawn on the render path and left 24 strings in a state no correct action could clear.

**Read the two i18n gates as the different things they are.** `i18n-locale-check`'s *30 locales ×
701 keys clean* is **structural** — parity, slots, plural categories, homoglyphs — and says nothing
about whether a value was ever translated. That second question is `i18n-untranslated`'s, and as of
2026-08-23 it is **an explicit key allowlist at target 0, not a baseline count**: the backlog it was
built to measure (51 keys verbatim English across every locale — the Registry Browser, Chat's
empty/prompt text, the site-directory verification sublines, the app-host failure messages) **is
translated in all 30 locales**, the baseline file is deleted, and the six survivors each carry their
reason in `ALLOWLIST` (three palette proper nouns, OPFS, IndexedDB, one format name).

It **cannot check the 17 Latin-script locales and says so in its own pass line** — correct German
often looks like English, so no mechanical signal separates a cognate from a skipped string. Those
were translated in the same pass and graded by eye; 17 values out of 1,470 remain identical to their
English source because the correct word is the same word (*Chat* in de/nl/it/cs/da/no/ro, French
*Source* and *Message…*, *byte* as a unit). **The two 17s are a coincidence** — 17 languages, 17
strings — and conflating them is what made the first write-up unreadable.

**Say which suite you mean, and re-run before quoting.** These numbers have gone stale in hours,
repeatedly. The 19 ignores are 4 `crossimpl_go_live` (needs core-go's live publisher), **13 fixture
emitters** the end-to-end suite drives with `--ignored`, 1 static-export demo emitter, and 1
live-backend upload — re-enumerated 2026-09-09, because this very sentence said "17" while two
succession fixtures had landed behind it, exactly as it said "8" before that. **Nothing keeps this
breakdown honest but re-running it**, which is the argument for reading the count off the suite
rather than off this file.
Note that `make test` compiles `tests/e2e_worker.rs` to **nothing** (`#![cfg(feature = "e2e")]`), so
a green `make test` is no evidence that file even parses — **`make lint` is what compiles it**, and
only since 2026-09-02. Before that, ~25k lines were type-checked by nothing but a Selenium run.

## What is proven, and on what

Every **gate** below is podman containers on a single Linux box, and each rig states its own scope.
That is a statement about the automated gates, **not** about the product — the same flow was run by
hand on two real devices on 2026-08-21 (see below the table). What no gate and no hand-run has
covered is two devices on two *different networks*.

| Capability | Gate |
|---|---|
| Browser ↔ browser chat over WebRTC | `make e2e-webrtc-chat` (mechanism) · `-meet` (shipped path) |
| Meet-by-name, browsers type nothing | `make e2e-webrtc-advertised` |
| Same-LAN, zero infrastructure (0 reflectors) | `make e2e-webrtc-lan` |
| Two real NATs, two external addresses | `make e2e-webrtc-traverse` |
| Negative control (no reflector ⇒ no media) | `make e2e-webrtc-nat` |
| Browser ↔ browser file transfer | `make e2e-webrtc-file` |
| Browser ↔ native Tori file transfer | `e2e_worker` Phase 14 / 14b — reads the bytes off the backend's disk |
| Tori **as** the signaling node, under enforcement | `src-tauri/tests/signaling_node.rs` |
| Port mapping (PCP / NAT-PMP) | `src-tauri/src/port_mapping.rs` |
| Name → registry → signed root → page, in a browser | `make e2e-federation` (3 containers, 3 distinct addresses) |
| Cross-impl: our reader vs core-go's live publisher | `make crossimpl-go` |

**The product thesis HAS been demonstrated on real hardware, and the line that used to sit here
saying otherwise was stale.** On 2026-08-21 the operator ran it outside any rig: **two browsers
exchanged chat over WebRTC and then transferred a file between them, rendezvousing through a
desktop Tori on the same network** — no peer id retyped, no harness, no `podman network create`.
That run is what exposed six defects no gate we own could see; four are fixed and two are listed
below as open.

**What remains untested is two *networks*, not two computers** — a peer behind one ISP reaching a
peer behind another, i.e. the port-forwarding / CGNAT / symmetric-NAT half. That needs a second
network nobody here has, so it is a post-release item waiting on hardware, not an open engineering
task. Same-LAN is proven on real devices; off-LAN is proven only in the container NAT rigs.

## Device verification — human, not agent-doable, and **none of it gates the release**

1. **Optimized bundle on Safari / iOS.** Engineering is done: the binaryen **119** pin in the
   `Dockerfile` fixes the reference-types funcref mis-optimization that threw `Table.grow`
   RangeError on JavaScriptCore, and `make wasm-release` builds clean through it. **Remaining:**
   deploy the rebuilt bundle and open it on a real iPhone and desktop Safari.
2. **IndexedDB across-restart durability on WebKitGTK — ✅ verified.** Confirmed by hand via
   `make tauri-run`: create a site, save, relaunch, the site is still there. Safari/iOS on real
   hardware still wants the same check; the WebKitGTK/JavaScriptCore question is answered.
3. **Offering a file on Android — ANSWERED, and the answer is "not ours".** Firefox for Android
   accepts a file-chooser request and closes it itself in ~200–250 ms without showing it; **Chrome
   on the same phone, same page, same file works**. Measured across all nine ways a page may open
   a chooser (hidden, rendered, visible-and-tapped-directly, script, `showPicker()`, `<label>`,
   shadow root) — every one is dismissed, so **no app-side change fixes it**. What was ours is
   fixed: the app listened only for `change` and never for `cancel`, so the failure was completely
   silent; it now says so. `CHANGELOG.md` carries it as a known limitation, scoped to *picking a
   file to send* — browse, pull and receive are unaffected. `tools/picker-probe.html` is the
   nine-row matrix, kept for the next device-only picker question.
4. **Full screen and wake lock in the Tauri WebView.** Both are Firefox-gated here. WebKitGTK is
   a different engine and has surprised this repo badly before (it ships no `RTCPeerConnection` at
   all). Both degrade honestly — the full-screen button renders only when
   `document.fullscreenEnabled` is true, and a wake-lock request on an engine without the API is a
   no-op the app already handles — so the outcomes to tell apart are *absent* and *works*.
   `make tauri-run`, open Apps, launch something, press ⛶ and leave it running.
5. **Two real devices on two real NETWORKS — blocked on hardware nobody here has, and note the
   same-network half is already DONE.** Two devices on one LAN, meeting through a desktop Tori and
   moving a file, was run on real hardware on 2026-08-21 and recorded step by step. What is
   untested is the cross-*network* case — one ISP to
   another — which needs a second network, i.e. a second physical location. Not an open engineering
   task; a standing item for whenever the hardware exists. What no rig on this box can produce: a
   phone on cellular, a captive portal, an ISP CGNAT, or a symmetric NAT.

## Commit pins — why this file has none

ADR-0027 authors every published commit **fresh at the release boundary**, so public history is a
different history from `dev`. A `dev` SHA has therefore never resolved for an outside reader and
never will — and repointing one at another `dev` SHA changes nothing for them.

This file used to carry **15** such pins, the largest single concentration in anything this repo
publishes. They are gone, replaced by the file, symbol, or measurement the citation was actually
resting on. Measured exposure across the repo is **48**, of which only 18 are countable from
`CANONICAL-DOCS.toml` — source comments and prose outside a `docs/` root are *always kept* by the
publish filter and appear in no manifest.

The durable form is keystone's: cite **content**, not commits.

## Backlog

Verified against the tree on 2026-08-23; items that had closed are struck rather than silently
dropped, because a backlog that only grows is not being read.

**Quick wins**
- `inspect tap` shell verb — ~30 LOC shortcut for `open Path Tap`; the last open item in the
  inspect verb set (the other 7 sub-ops shipped).
- `Peers::sdks` Vec compaction — there is no `detach_worker_sdk`, so deleted `Backend*` peers
  leave an empty SDK slot until reload. Gated on upstream `WorkerProxy::terminate()`.
- ~~Clippy nit in `src/views/shell/binding.rs`~~ — **closed**, no longer present.

**Performance (ranked, ready)** — `docs/architecture/reviews/PERF-ANALYSIS.md` §7
- **Entity Tree local-state refactor** — the biggest single win, and it establishes the per-window
  `HashMap<path, hash>` pattern the others copy. Measured 381–655 `get_entity` calls per render on
  a 281-row tree, 12.6 ms avg / 25 ms max, over the 16 ms frame budget.
- Knowledge Base article-list refactor — a mechanical copy of that pattern.
- Event Log + Query Console shared `CachedEventLog` ring buffer.

**Security — deferred, not needed for the first release**
- **Frame-scoped CSP so the main app can be strict.** Today the app CSP is loose under Tauri
  (`script-src 'unsafe-inline' 'unsafe-eval'`) and absent in the browser, because sandboxed
  `srcdoc` app iframes inherit it and need inline scripts. **Trigger to do it:** when users
  routinely browse untrusted third-party peers' sites. **Fix:** serve app bundles over a custom
  URI scheme carrying its own permissive CSP, so the frame stays loose while the main app goes
  strict.

**Shell extraction**
- Tier-E verbs still open: `revision`, `history`, `role` (SDK ops landed, verbs unwritten).
- Standalone `entity-shell` binary + one-shot `dls` / `dcat` / `dexec`.
- Persistence-helper consolidation, `src/app_paths.rs` vs crate helpers.

**Open product decisions — need sign-off, not a unilateral call**
- Stage-A2 tree search is half-wired: `set_search` exists, `flatten_visible` ignores it. It is a
  cross-impl shared-shape question with `entity-workbench-go`.
- `src/action_event.rs` keep-or-delete — still zero callers *by design*, as a cross-impl schema
  anchor. Verified still true.
- Query / count / execute hard-code the primary peer in `src/app.rs` — a latent peer-scoping bug. Fix
  is to thread the window's `peer_id` through `Action::Query` / `Count` / `Execute`.

**Persistence**
- Offline-wipe: a hard refresh while the server is unreachable wipes local state. Needs a
  hash/version handshake and an offline-keeps-local design.

**Connectivity & liveness** — the wake arc. Items A and B shipped 2026-09-08; the rest is open.
- **THE UI FREEZE AFTER WAKE — unexplained, and it needs a reproduction, not more analysis.**
  Reported 2026-09-07/08: on wake the app *"would not close windows"*. A mechanism is identified —
  a `borrow_mut()` held across an await makes the rAF loop's `try_borrow_mut()` fail every frame
  and log `FRAME SKIP`, which is exactly "alive but unresponsive" — but **both known instances are
  boot-only** (`main.rs:722`, `main.rs:845`), so that is a class to look for, *not* the diagnosis.
  The other six `borrow_mut()` sites in `src/` were checked and do not hold across an await.
  **Note it is not merely cosmetic: the frame loop drives `reach_keeper::pump`, `ChatDelivery`'s
  poll and now `wake_probe::note_frame`, so a wedged UI disables every app-tier detector at once**
  — which is why the freeze and the connectivity symptom may be one bug rather than two.
  **Evidence to capture when it recurs, in this order** (they are three different bugs):
  `FRAME SKIP` ⇒ something holds the app `RefCell`; `FRAME STALL` (>50 ms) with no SKIP ⇒ the frame
  itself is slow, not blocked; `FRAME PANIC` ⇒ a panic unwinding out of `frame()`. Then the Event
  Log's watchdog note, which since 2026-09-08 names *which* of the four causes fired.
- **The sleep/idle class has NO browser gate, and this is the blocker for honestly closing the
  arc.** `make e2e-webrtc-idle` was written for exactly Amendment 14's idle-death case and
  **exits 2 = INCONCLUSIVE** — by the Makefile's own note, `ChatDelivery`'s 5 Hz poll keeps the
  link busy so nothing ever goes idle. Making it meaningful means retiring that poll onto
  `reach_keeper` (its module doc proposes it), which is the **A3 investigation**, not a cadence
  tweak: the poll is load-bearing for establishment retry and for symmetric delivery over WebRTC.
  Until then, do not let a green suite imply coverage this class has never had.
- ~~**A VANISHED PEER IS OFTEN NEVER NOTICED**~~ — **CLOSED 2026-09-08.**
  `make e2e-webrtc-vanish` now reads **6/6 PASS at 0.5 s** on fresh rigs, across both handshake
  roles, where it was ~half at 30.6 s and the rest never within 75 s. Found by the gate written to
  prove the opposite, landed red for a day, and closed by **two `core/peer` fixes** — the budget
  (12 s) never moved.
  **Both fixes are one rule at two sites: a transport primitive that EVICTS its own binding
  disarms the §A1 demotion.** `demote_peer_on_transport_error` fires only while the failed
  endpoint is still bound, so evicting from the transport makes the next dispatch's demotion a
  no-op — and `escalate_unbound_suspect` escalates only from `suspect`, which nothing then wrote.
  Measured: the status entity is left reading `connected` with nothing able to move it, i.e. worse
  than the bug. So each primitive now **reports** and the seam **acts**: a dispatch issued after
  the reader has ended fails immediately rather than at the 30 s deadline, and the binding is left
  in place for the seam to evict and demote.
  **The bimodality was ONE defect with TWO code paths.** A §6.5 link is one connection carrying two
  handshake roles — the offerer becomes the §7.4.1 initiator, so one side runs the dialer's reader
  loop and the other runs the accept loop — and peer ids are fresh each run, so the role flipped
  run to run and a deterministic gate scored 50/50. (The offerer rule decides the handshake role,
  **not** which side dials: both ICE agents fire outbound.) The gate's
  log panel now prints on PASS too, with role needles, so a future run says which mode it took.
  The two kernel changes are `RemoteConnection::reader_ended` (the dialer half) and
  `InboundReentryEndpoint::connection_over` plus the accept loop's teardown no longer
  deregistering (the acceptor half), gated by
  `a_dispatch_after_the_reader_ends_fails_now_not_at_the_request_deadline`,
  `the_reader_ending_leaves_the_binding_for_the_a1_seam_to_evict`,
  `a_reentry_dispatch_after_the_accept_loop_ends_fails_now_not_at_the_request_deadline` and
  `an_acceptor_notices_the_dialer_that_vanished`. Detail, including the two wrong turns it cost,
  is in this repo's agent guidance.
  **What the earlier diagnosis got wrong**, kept because it is the useful half: the second mode
  was attributed to session lifecycle in the browser's WebRTC proxy, and it was the acceptor path
  in the kernel; and the fix shape it proposed — evict on reader exit — is the mistake the first
  paragraph describes.
  **AUDITED ACROSS THE COHORT, and all three implementations reach the same stuck state by three
  different routes.** The acceptor half is live in the Go implementation (source-verified: its
  accept-loop teardown deregisters the same reentry binding its own no-clobber guard then requires,
  and an acceptor-only binding there has no keepalive loop at all). The Python implementation is not
  the same defect and ends in the same place: its acceptor writes `connected`, and its demotion is
  gated on evicting from the **outbound** connection pool by object identity, so a peer it never
  dialed cannot be demoted by any path — Go has an arm its own teardown disarms, Python has no arm.
  A rule two implementations get wrong is a coincidence; a rule all three get wrong by different
  mechanisms is a specification that does not say the thing.
  The Go implementation had *already* reached our dialer-side fix independently, which is
  the argument for promoting that behaviour: today it is only an informative SHOULD in the core
  protocol's teardown contract. And the composition the defect lives in — demote-only-if-still-
  bound, escalate-only-from-suspect — has a hole at a site the specification never mentions:
  connection teardown. Routed to arch and to the Go implementation with the asks spelled out.
  **Not closed by this: on the acceptor side liveness is corrected by a DISPATCH, not by the
  teardown.** A peer that accepts a connection, never dispatches back, and whose counterpart
  vanishes still renders `connected` — true before this change too; the difference is that it is
  now correctable rather than permanent. A browser always dispatches, which is why the gate
  closes; a server-role peer that only ever answers does not.
- ~~**Item C — no `connectionstatechange` handler**~~ — **shipped 2026-09-08**, in
  `entity-core-rust` `bindings/wasm-worker-proxy/src/webrtc_session.rs` (**quote the pair**: our
  `dev` + the kernel commit it was built against, which the shipped page stamps as
  `entity-core-ref`). The data channel's `close` and the peer connection's
  `connectionstatechange → failed` now post the zero-length EOF sentinel `PortReader` already
  surfaces, so a dead channel fails the connection immediately instead of on the 30 s
  `DEFAULT_REQUEST_TIMEOUT` — or, when idle, not at all. `Disconnected` is deliberately **not**
  terminal (the spec makes it transient; it escalates to `failed` on its own via RFC 7675 consent
  freshness). Verified not to fire on healthy paths (chat ×3 fresh-grid, meet, file 700 KB, EOF
  marker absent throughout).
  **CORRECTION, same day: its commit message overclaimed, and was then made true.** It said the
  connection "fails now rather than on the next 30s request deadline"; measured, it did not — the
  sentinel is posted and nothing downstream acted on it. The two `core/peer` fixes above supplied
  the missing half, so the claim now holds. The EOF wiring is **necessary and was not sufficient**
  on its own, which is the shape to remember rather than the sentence.
  The wiring is a prerequisite for that claim, not the claim. See the open defect above, which is
  the gate that caught it.
- **Item D — a visible reconnecting state.** Amendment 12 ruling D explicitly **rejected** a
  `connecting`/`reconnecting` value on the liveness entity and routed it to the derived
  `system/network/peer-summary`; `dial_markers` already does this for the *first* establish. So the
  work is extending a derived read-model, **not** adding a state. Needs i18n across 30 locales
  (`i18n-lint` baseline is empty — `raw=0` — and must stay that way).
- **`MeetReach::NoNode` / `NeedsReload` still tell the user to reload.** The late-arm fix
  (`src/late_establish.rs`) made that advice obsolete on the Direct arm; it remains correct on
  Worker. The same
  stale row survives in `RUNBOOK-TWO-MACHINES` §5 (`FAIL rendezvous … reload the page`).
- **The establisher is primary-only.** Nothing technical requires it — an artifact of the seam
  being a constructor argument on the primary's keypair, recorded in `peers.rs` as deliberate
  policy.

**Content / publishing**
- **Cross-domain `site:` links — the fail-loudly half is CLOSED on both sides.** Measured
  2026-08-23: the corpus carried **seven**, three files, all on `entity-church-foundation`, all
  outbound; under per-domain publishing they resolve against the publishing peer and 404.
  `entity-core-papers` swept them to absolute URLs and added its own gate; our
  exporter **reports** every out-of-set target and now **refuses by default**. Re-verified here
  rather than taken on report: **0** cross-domain refs across all five domains, all four domains
  publishing clean under the refusal (412 pages, 0 dangling), an injected target refused with 7
  errors and exit 1. Hatch: `ALLOW_OUT_OF_SET_LINKS=1`; `STRICT_LINKS=1` is accepted and is now a
  no-op. Making such a link *work* is backlog **B-4**, and it is a protocol question before it is
  an implementation one.
- **Latent, filed not fixed:** the Registry Browser writes a resolved origin under
  `system_peer_id()` while the Site Browser reads under its **bound** peer. They coincide only
  because `system_peer_id()` is still an alias for `primary_peer_id`; a Site Browser on any
  non-primary peer gets `Unreachable` on a name that resolved. Same shape as the registry-pin bug.
- **B-3 is now LOAD-BEARING rather than optional — its trigger fired 2026-08-24, and it is still not
  release-blocking.** Raised against this repo's own rule and confirmed here:
  the trigger is *"ANY path by which bytes reach the renderer from an origin the deployment did not
  supply"*, and `open_in_site_browser` now registers an origin that came from a **signed registry
  binding**. Four things, and the third is what keeps the severity honest:
  - **The thing the rule most feared did not happen.** `verified_at` is still hard `None` with the
    reason inline, and every foreign rail row still says *not verified*. The D2 violation — a
    verified *resolution* laundering an unverified *fetch* — has not shipped.
  - **The exposure did not change in kind; the origin SET widened**, from the deployment's list to
    that list plus anything the pinned registry names. What moved is whose judgement bounds the set:
    the deployment operator, and now also the **registry operator**. Today the same people.
  - **A registry-resolved origin is NOT an unvetted one**, which is why "prerequisite" overstates it:
    it arrives inside a binding signature-checked through the registry's signed root, with the D1
    name check, a non-null TTL and the §6a.6 revocation probe — *better* provenance than a
    deployment-config origin, which is an unsigned JSON file. The residual is narrow: **a host named
    by a legitimate signed binding serves bytes the publisher never signed.**
  - **What is spent is the ARGUMENT, not the labelling.** *"The deployment decided every origin in
    advance"* is no longer available; the replacement is trust in the registry operator. **When that
    stops being us, this stops being a deferral.**
- **Two peers publishing one site is safe in the data and ambiguous on screen — measured
  2026-08-24** against a proposed consolidation, and the measurement is why we advised against it.
  No corruption, no shadowing: the rail dedups on `(peer, site)`, sorts the twins **adjacent**, keys
  prefs/provenance by the same pair (so bookmarks and counts do not bleed), compares both fields for
  the current-row highlight, and sends the concrete foreign peer on click — over content-addressed
  blobs at peer-namespaced paths. **The cost is presentational and lands exactly where B-3 bites:**
  the two rows share a title and are distinguished **only by host**, a transport fact, with
  `verified_at` `None` on both. Standing advice, agreeing with `cgid-10-235` (*one domain = one
  identity*): **do not consolidate** — the registry replaced the need to, and a signed
  `name → peer-id` makes serving another domain's content under this key a signed **authorship
  claim**.
- **`last_reconciled` is on `SiteEntry` and rendered nowhere** (zero hits in `dom/site_directory.rs`),
  so the rail carries no freshness signal at all. Latent generally; it is the missing tiebreak in the
  duplicate case above. Small, real, unscheduled — and note the `N×` tail is **`visit_count`**, a
  usage counter with no provenance meaning, which has already been misread once as a provenance one.

**Build & release**
- `dist/` hygiene — ship `make wasm-release` with default features, never a debug or `demo-apps`
  `dist/`.
- Vault label `|` / newline is not escaped (`vault_codec`); do it in a calm window alongside input
  validation.
- ~~Commit `Cargo.lock` for reproducible release builds~~ — **closed**, both lockfiles are tracked.
- **A build of this application is identified by a PAIR, and only one half is pinned outside CI.**
  This crate links the Rust reference implementation by **path dependency** across twenty paths, and
  there is no cross-repo lockfile — so the bundle hash is a function of this repo's commit *and* of
  whichever sibling checkout was on disk when the bundle was built. Two things are now true and one
  is not:
  - **Recorded.** A built shell stamps `entity-build` (this commit), `entity-core-ref` (the kernel
    commit it linked against) and `entity-build-id` (the bundle hash, which is the *identity* — a
    commit is only a label). Either stamp reads `-dirty` when its tree was dirty, and `unknown` when
    it could not be read; *"we could not tell"* and *"nobody recorded it"* are kept apart on purpose.
  - **Observable.** `make fleet-probe` reports `pair=(commit, core-ref)` per domain, prints the pair
    beside each build when a fleet is not uniform — so *"same commit, two different bundles"* names
    its own cause without a bisect — and calls out a dirty half as bytes no commit can reproduce.
    Reported, never the verdict: identity stays the bundle hash, because two kernel commits can
    legitimately yield one bundle when the difference did not reach this crate.
  - **Controlled, as of 2026-09-08.** The release workflow already pinned the kernel ref; the
    **local** build path — which is how the web tree is built — did not. It does now: emitting the
    uploadable tree **refuses before it builds** if either half of the pair is a dirty or unreadable
    checkout, prints the pair in its summary, and accepts an explicit kernel ref that it *verifies*
    (never checks out — another repository's history is not this build's to move). The waiver for a
    dirty tree deliberately does **not** waive a mismatched pin: those are different problems and
    the way out of the second is to stop asking for a pin.
  - **What that is and is not.** It makes the pair *meaningful* — both halves are at some commit, so
    a reader can reproduce the build by checking the pair out. It is **not** a cross-repo lockfile
    and it does not choose a version for you. That remains open.

  Why it is here rather than in a note: this was measured, not theorised — a comment-only edit to an
  unhashed asset appeared to move the bundle hash, which our own rules say is impossible. Rebuilding
  twice at a fixed commit returned the same new id, which separated *"the build is
  non-deterministic"* from *"an input nobody was tracking changed."* It was the sibling.

**Long-deferred capability stages** (`docs/architecture/specs/SYSTEM-VISION.md`): KB wiki PoC, type renderer registry,
pipeline builder (SDK Layer 2), relay on the Tauri backend, the capability + identity arc (Key
Manager stays a placeholder until then), cross-renderer portability, self-modification. Pull into
the roadmap when scoped.

## Next — **post-release**, and deliberately not started

The release ships from where the tree is. Everything in this section was reviewed on 2026-08-23
and **held**, not because it is unimportant but because the team is at the end of a long push and
opening any of it now trades a shipped release for a wider one. The intent is a **shorter** cycle
after this — closer to a week than to the two months this one took — so this list is the input to
that, not a set of loose ends.

0. **Peer-to-peer save transfer — the one that was nearly pulled in, and the design question that
   stopped it.** `send_save` / `scan_peer_saves` / `import_save` (`views/games/mod.rs`) are
   **built and work browser↔browser**, riding the ordinary offer/pull path: a `SaveBundle`
   published as an offer, candidates identified by *decoding* rather than by filename, and an
   incoming save backing up whatever it replaces. **It has no gate at all** — `make test` covers
   only the local half (list / backup / restore / bundle round-trip), the two cross-peer halves are
   `#[cfg(target_arch = "wasm32")]` so no native test can reach them even in principle, and the e2e
   asserts only that the Saves panel opens.
   - **The blocked leg is the desktop one, and it is blocked for a known reason:** the Linux
     desktop WebView ships **no `RTCPeerConnection` at all**, so offer/pull is
     dead in both directions browser↔WebView — each end would have to dispatch at a peer it cannot
     reach.
   - **The shape that does not need a relay, and is the thing to try first:** both parties can read
     and write the **backend's share** over a plain WebSocket — the browser by dialing it (proven
     daily by `e2e_worker` Phase 14b), the WebView because it already dials its own backend. So
     getting saves *into Tori* — or backed up to the native peer on connect — is a second **sink**
     and a second **source** for `SaveBundle::to_bytes` / `from_bytes` / `import_save`, all of
     which already exist. Not new transport; new endpoints on transport that works.
   - **Cheapest proof if it is picked up:** `make e2e-webrtc-file` already stands up two browsers
     that meet and move a file, so a save phase is a variation on an existing rig — that is the
     browser↔browser half. The Tori half is the share, not the rig.
1. **`entity-workbench-go`'s app tier.** Its kernel is ready — core-go has signaling (punch, pool,
   coordinator, node), the §10.3 seam with single-flight, and srflx — but the app tier has none of
   it. It needs the four pieces we built here: a liveness read-model, a `maintain-peer` driver,
   transport-profile publish, and a connector registry + meet. **No WebRTC needed** — Go has no
   stack and does not need one to interoperate over WebSocket. **The prize:** every P2P gate we
   own is rust-browser ↔ rust-browser, which is *cohort-consistent, not independent convergence*
   (ADR-0012). This buys the first genuinely independent evidence for the product surface.
2. **A Python interop probe.** `entity-core-py` has a very complete handler set and no app tier,
   which makes it the cheapest far side for an existing transfer gate — it already has `content`,
   `local_files` and `tree`, so it needs no app tier at all.
3. **Relay (`EXTENSION-RELAY` Mode S).** Landed, implemented upstream, builds for wasm, and we
   consume none of it. It is put-then-poll, so it needs no rendezvous and no simultaneity — the
   third rung of reachability, and reachable today from any party that resolved our name.
4. ~~**TURN / relay credentials**~~ — **closed.** `Connector.relay` / `.relay_username` /
   `.relay_credential` ship, with a second `IceServer` on the way to the agent and
   `E2E_RELAY*` on `e2e-webrtc-meet` as the gate.
5. **A registry binding's `ttl` is doing two jobs, and the spec defines it both ways. Known,
   understood, and deliberately held for arch after the release** (operator's call, 2026-08-24).
   - **The finding.** `EXTENSION-REGISTRY` §2.1 declares `ttl` a *"positive-result **cache
     hint**"*; §3 — which §2.1 itself names as canonical — says *"null = sticky until revoked"*;
     §6a.1a makes it *"the only bound on a withheld revocation"*. A cache lifetime and a validity
     duration are different parameters. **We implement only the validity half**, measured:
     `resolve_one` refuses on `issued_at + ttl <= now_ms()` and the resolver clamps, while
     `effective_ttl_ms` has exactly two consumers in `src/` and **both only display it** — nothing
     caches a resolution anywhere.
   - **The operational cost of the conflation, which is the reason it is on this list at all:**
     the TTL is the only thing that ever stops a *withheld* revocation being believed, so a shorter
     revocation window can be bought **only** with a more frequent republish. They are one number.
   - **DNS is not the outlier, and this is the part worth not re-deriving.** DNSSEC carries both
     (record TTL for cache, `RRSIG` inception/expiration for validity); X.509 carries both
     (`notBefore`/`notAfter` plus OCSP `nextUpdate`); JWT carries `exp` plus a separate
     introspection policy. Plain DNS needs no validity half because it has **no revocation concept
     at all** — it has *fewer* parameters, not a different model.
   - **The fix is already scoped upstream and we are not proposing a mechanism.** The protocol
     side's own exploration of non-interactive freshness names this seam
     (*"canonical and cross-referenced but un-quantified"*) and its **Knob 2** is exactly this
     split — a declared `revocation_propagation_bound` making the window `min(TTL, declared-bound)`.
     Part E puts it at **W7, not before-freeze**; the recommended proposal is **not written yet**
     (checked — only the exploration and its companion analysis exist). What we asked for is the
     narrow half: reconcile the §2.1-vs-§3 text, and confirm which job an engine must implement.
   - **The knob, and where it actually lives.** `--ttl-days=N` →
     `make registry TTL_DAYS=N` (`Makefile:1507`) → the publishing pipeline's own
     `REGISTRY_TTL_DAYS`. Fully wired end to end;
     **no code change is owed by anyone** to change the value. There is no protocol maximum — §6a.9.2
     explicitly declines to set one — and both existing ceilings (issuer-policy `max_ttl`, the
     resolver's local ceiling) bound it downward only.
   - **Trap worth stating: our `DEFAULT_TTL_MS` is dead for production.** The publishing pipeline
     always passes `TTL_DAYS` explicitly and carries its own default of 30, so editing our constant
     changes nothing that ships.
   - **What it does *not* cost when bindings lapse**, measured 2026-08-24: sites a visitor has
     already opened keep working. Origin registration persists independently of the binding, so
     expiry stops *new* name resolution and nothing else.

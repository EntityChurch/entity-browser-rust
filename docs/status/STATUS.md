# entity-browser-rust — status

_Updated: 2026-08-18 · public: v0.8.0 (master) · working branch `dev`, **version 0.8.2**_

> **Bearings first:** `HANDOFF-2026-08-18-the-naming-chain-is-built-and-what-it-waits-on.md` —
> the publishing/naming trust chain is **built end to end** (publish signs → the browser verifies
> → a registry names → a name resolves to a verified page, four domains, nothing live), what is
> blocked on arch, and what is next-ready. Routed in `ROUTING-2026-08-18-b` (+ `-a`).
>
> **Latest:** `STATUS-2026-08-18-c-validated-in-a-browser-and-F1-is-closed.md` — **everything is
> measured**: `make e2e-worker` **18/18 unfiltered (319.69s)** including a new gate that resolves a
> name **cross-origin in a real browser** and verifies the page it names (RUNBOOK §4, the vector
> assigned to this repo). **F1 is CLOSED** — core-rust `a23bb27` landed arch's D1/D3/§6a.6 and three
> of our tests flipped from demonstrating the defect to guarding the fix. Two corrections in there
> too: earlier gate numbers this session were a **cached** build, and one cross-origin failure was a
> **port squatter**, not the app.
>
> **Also:** `STATUS-2026-08-18-b-the-naming-flow-end-to-end-and-a-browser-can-now-drive-it.md`
> — the whole naming flow written out (three peer-ids, two of which matter; publish-time files;
> two-hop resolve), plus two firsts: a **`name` shell verb** so a browser surface can actually drive
> `resolve_name` (it had **zero callers**), and the chain running **over a real socket with CORS** —
> 4 domains, 35 requests, all 200. Still never run in an actual browser; that is the next gap.
>
> **Also:** `ROUTING-2026-08-18-c-we-wrote-the-globs-proposal-and-a-resolve-never-sends-the-name.md`
> — B16b's blocker was arch's `name_format_dispatch` globs; **we wrote the proposal** rather than
> keep waiting (ours to draft, arch's to ratify). It turns on a measurement: resolving through a
> signed root **never puts the name on the wire**, so the catch-all's safety is a property of the
> *backend*, not the glob. Mechanism shipped + mutation-checked; the default globs are installed
> nowhere pending sign-off.
>
> **Also:** `STATUS-2026-08-18-a-a-signed-root-enumerates-but-a-withheld-node-is-silent.md` —
> arch closed our three §9 items and disposed all four findings (`ROUTING-2026-08-18-b`). Their F4
> ruling is confirmed by measurement (a signed root **is** enumerable) and **its detection half is
> refuted**: a withheld interior node shortens the walk with no error. arch D10 closed on our side —
> a transportless `peer-issued` binding is now refused at the emitter.
>
> **Gates at this commit:** `make test` **1113/0/4-ignored across all 15 test binaries**
> · `make e2e-worker` **18/18 unfiltered, 319.69s** · (the main binary alone is 1016/0/3 — the older `995` and `1059` figures here were the main
> binary and a 14-binary run respectively; say which you mean) · `make lint` clean (30 locales ×
> 571 keys) ·
> `make wasm` check-dist consistent · `make federation` stands up and `--verify`s a four-domain
> deployment. **e2e re-run and green: 18/18 unfiltered, 319.69s** (2026-08-18).
> `e2e-webrtc-meet` / `-file` last measured PASS.
>
> **Older bearings, still useful for the connectivity/sharing threads:**
> `HANDOFF-2026-08-17-b-bearings-three-threads-and-what-is-ours.md`. Gate figures marked
> `1035/0` or `1045/0` anywhere below are **stale** — re-measure before quoting.
>
> **Track B — the re-release arc** (publishing pipeline, app catalog, save slots, the release
> itself, the user flow): `reviews/BUILDOUT-THE-RE-RELEASE-ARC-PUBLISHING-PRODUCT-AND-RELEASE.md`.
> It runs in parallel with the connectivity/sharing thread and is entirely ours. Its §8a–§8c now
> carry the signed-root, registry-emitter and full-path work.

## Where it is

`entity-browser-rust` is the DOM-primary Rust/WASM **reference application** on the
entity-core substrate: a window-manager / UI shell over the entity tree, rendered as HTML
DOM in the browser (WASM peer) and in a Tauri desktop WebView. It is a *binding / app*
built on the Rust reference implementation and its SDK — one worked example of the
paradigm, not a mandate. Maturity is **v0.8.0 research preview**: suitable for evaluation
and exploration, not a hardened production deployment. The legacy native/egui renderer has
been removed; HTML DOM is the only render path (`make native` prints a deprecation
redirect). Building green (`make wasm` produces both `entity-browser-*_bg.wasm` and
`entity-worker_bg.wasm`; `check-dist` consistent).

## Current arc (2026-07-08 → 2026-08-15) — connectivity, and it is no longer the blocker

The work since the release arc below has been one continuous thread: **make two peers on
different machines actually reach each other**, and make every surface that claims they are
connected be telling the truth. It is done to the point where connectivity is no longer what
blocks the product.

**Where connectivity stands — four gates, each proving something different:**

| gate | proves | status |
|---|---|---|
| `make e2e-webrtc-traverse` | media crosses **two separate NATs** via a reflector | ✅ |
| `make e2e-webrtc-nat` | negative control: no reflectors ⇒ no media | ✅ |
| `make e2e-webrtc-meet` | the **shipped** path: name → id → connection → message | ✅ |
| `make e2e-webrtc-chat` | the §6.5 mechanism | ✅ |

`traverse` + `nat` together are what separate *"ICE is configured"* from *"ICE works"*; neither
alone is enough. Re-verified at HEAD 2026-08-16 alongside `make test` **1026/0** (the total across every test
binary — the bin's own unit tests are 944 of those), lint, wasm.

**The load-bearing changes, in the order they mattered:**

1. **Liveness is kernel-owned.** The app-tier `connection_health` mirror — which guessed from
   connect *attempts* and reported "Connected" straight through a mid-session drop — is
   **deleted**. Every surface now subscribes `system/peer/status` through the `peer_liveness`
   read-model, and `watch_all_vantages` made the subscription one call instead of three
   hand-rolled loops. **Reachability** followed the same way: a successful dial publishes a
   kernel transport profile, so `connections.rs` is a petname/authz registry and **not** an
   address book. Authoritative: `MODEL-REMOTE-PEER-FACTS.md`.
2. **Reconnect is the EXTENSION-NETWORK driver**, not a hand-rolled loop — `maintain-peer`, with
   engines started per-local-peer by a frame sweep, and `release-peer(idle)` on unbind that
   stops the retries without hanging up.
3. **Meet at a name** — a connector registry on the system peer, `meet tag <label>`, and the
   discovery/reachability split made honest (a peer with no establisher warns before handing
   out an id it cannot be reached on).
4. **ICE has a source.** The provisioning pipe already existed end-to-end; what was missing was
   an input box. `Connector.ice` is it — a reflector belongs to the same operator as the node
   you rendezvous through. `stun:` only; TURN is refused **with its reason** (no credential
   carrier yet). Never a public default — that would enrol a third party invisibly.
5. **A sequential bound on §10.3 seam consultations** (upstream, `entity-core-rust`): an
   unreachable peer produced ~974 negotiations per 196 s at 5 Hz, each individually conformant.
   Now 28. Healthy path byte-identical.

**The most expensive lesson, worth reading before the next rig:** the traversal rig was red for
most of a session and two app-level blockers were reported upstream — both were symptoms of a
defect in **the rig's own NAT**. A thirty-line bare UDP hole punch settled in one step what an
afternoon of theorising could not. Now catalogued as **AP24**
(`DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md`) and reproduced in the rig's own self-probing
controls. Full account: `HANDOFF-2026-08-14-nat-traversal-works-and-what-it-cost-to-learn.md`.

**Known holes, stated rather than implied:** the traverse rig models a **cone** NAT — symmetric
NAT is untested and needs TURN; the `…/transport/websocket` profile type is missing upstream (we
write `tcp` for `ws://` and it works because the resolver returns the scheme); transport-profile
staleness is unmanaged (a saved LAN address dies when the machine changes networks, and
"offline" is not "wrong address").

## File-transfer arc (2026-08-16) — two browsers, two devices, one file

The connectivity thread's payload. **A browser peer can now SERVE a file**, not just consume one:
`src/file_offer.rs` chunks locally, ingests blob+chunks into its own `system/content`, and
publishes a manifest that gives the hash a filename. The receiver walks the closure. No transport
is named anywhere in it — the same code path rides a WebRTC data channel or a WebSocket.

| gate | proves | status |
|---|---|---|
| `make e2e-webrtc-file` | two browsers meet at a name, one offers, the other pulls — **18 checks**, incl. the window making the offer and a withdrawal seen on the far side | ✅ |
| `MODE=worker make e2e-webrtc-file` | the same, on the **Worker arm**, and asserts the arm itself (**19 checks**) | ✅ |
| `e2e_worker` Phase 14b | the **desktop path**: browser ↔ Tauri backend over `local/files`, ending in a byte-for-byte read of the uploaded file **off the backend's disk** | ✅ |
| `e2e_worker` Phase 14.2 | offer / withdraw on the Worker arm, no display, no peer | ✅ |
| `a_lone_file_transfer_window_lists_what_it_offers` | the window's own subscription — with only that window open, because the monolith cannot see subscriptions (below) | ✅ |

**Presence is the idea worth carrying.** A browser peer has no listener: it cannot be connected
*to* passively, so being reachable is an **activity it performs** — `reach_keeper` holds a
standing intent and probes while the kernel says a peer is not connected. It is
exchange-agnostic; chat had been getting the same thing for free from its 5 Hz delivery poll, by
accident.

**Known holes, stated rather than implied.** An offer is **permanent** — `system/content` has
`get` and `ingest` and no forget, and `handle_get` serves by hash without consulting the §6.4.2
binding, so withdrawal removes discovery, not access (measured: 5 bound entities for a 4-chunk
offer, 0 reclaimed; routed as buildout item 21). Offers are **open-posture only** — no grant
profile covers them and a browser peer cannot author policy on its own tree, so
`ENTITY_BROWSER_ENFORCE` fails them closed with no way to open them (buildout item 22, and now
§9 of `PLAN-OF-RECORD-capability-enforcement`). One file is capped at **16 MiB**, an offer-side
memory bound that wants a streaming ingest rather than a bigger number. And **a green
`e2e_worker` is not evidence that a surface subscribes what it reads**: every window is open
there, two of them subscribe the whole peer tree, and the Worker proxy's cache is a union over
all mirrors.

## Release arc (2026-07-02) — post-release: surfaces, republish, desktop content-baking

Working the post-release list toward a **billslab.com re-release**. On `dev`,
**pushed to origin**; each step gated green.

**Desktop content-baking — sites + apps in Tauri (latest, all verified live):**
Proved and cleaned up the offline desktop path end-to-end. Tauri embeds `../dist`
and serves it same-origin (no server), so a static publish into `dist/` ships in
the binary. Landed: (1) `3ccea8c0`/`67421984` **`make tauri-bundle`** —
wasm-release → publish sites/apps INTO dist/ → embed (plain `make tauri` can't:
its wasm-release wipes dist/); `INGEST`/`APPS_DIST` take **any path**, auto-staged
into the repo for the publish container. (2) `be5573bf` **apps now run in Tauri** —
the strict CSP blocked the sandboxed `srcdoc` app iframes' inline scripts (`+
'unsafe-inline' 'unsafe-eval'` to `script-src` **and** `"script-src"` in
`dangerousDisableAssetCspModification`, else a Tauri nonce nullifies it — the
grayscale-bug trap). (3) `c9e1c586` **WebKitGTK IDB durability VERIFIED** (create
→ restart → survives) + Deployment Guide **§8.1** documents the whole flow.
Verified live on WebKitGTK: cold boot fetches the baked deployment config + 3 site
manifests same-origin; games list, launch, and play. **Gotcha:** a returning app's
durable config wins over the baked `entity-deployment.json` — re-test a bundle
from a fresh profile (`rm -rf ~/.local/share/systems.entity.browser`).

**Footgun cleanup (see
[`HANDOFF-2026-07-02-FOOTGUN-CLEANUP.md`](./HANDOFF-2026-07-02-FOOTGUN-CLEANUP.md)):**
worked through the open bug list. (1) `de6bddec` demo now self-demonstrates
**cross-site nav** — a companion `demo-notes` site + reciprocal `site:` links,
proven by an e2e phase. (2) `cbf338a9` **surface hardening** — the status-bar
site toggle is suppressed on a Window boot even if a deployment mis-emits
`show_toggle=true`. (3) `2a63435f` publish **warns on dangling nav links** (the
delete-a-page 404 footgun). (4) `e6f39e92` doc: the static SSG `.html` path is
**content-store-free** (only the `.bin` form uses the two-hop store; the default
publish writes both). Note: **blue-green A needs no papers team** — it's provable
on the demo set; only the billslab dry-run needs papers' canonical markdown. This
first release is a **full wipe**, so republish/blue-green is off its critical path.

**Landed (earlier same day):**
- **Surface-axis refactor** — killed the `full`/`site`/`strict-site` **profile
  presets**; startup posture is now the two real axes set directly: `surface`
  (chrome / site / window + `window_type`) + granular `site_mode` /
  `peer_creation_enabled`. Touched config, Settings, deployment JSON, build knob
  (`ENTITY_PROFILE`→`ENTITY_STARTUP_SURFACE`), publish flags
  (`--config-profile`→`--surface`/`--window-type`/`--locked`). Gate: 607 native +
  make test, wasm + clippy clean, e2e 12/12.
- **Republish/blue-green design** — empirically-verified republish guide
  (`docs/architecture/guides/GUIDE-REPUBLISH-AND-INCREMENTAL.md`) + the design for
  safe large-deployment updates
  (`docs/architecture/reviews/DESIGN-REPUBLISH-BLUE-GREEN-AND-CONTENT-STORE-SPLIT.md`):
  shared append-only content store + versioned trees + atomic cutover, the
  scope-rooted `--prefix` mapping, the content-scoping axis (global-dedup vs
  per-peer; the tree answers *references*, not *store contents*), and the billslab
  redeploy runbook + compression findings.
- **Bug fix — `--surface=window` now disables the overlay** (`site_mode
  enabled:false, show_toggle:false`), so a Site Browser deployment shows **no
  stray "View Site" toggle** (which dropped a fresh peer into a missing-home
  overlay → `No site manifest at 'demo'`). Cross-site nav in the window is
  surface-internal (`navigate`→`go_to`), so the overlay is safely off.

**billslab.com reality check (inspected live):** healthy on both arms; root
deploy, no scope prefix; peer `2KD9pbm…`; ~985 pages (methodology 799) = the
2-hour scale → blue-green required. **The live config still says
`profile: "tutorial"`** (the new build ignores it) → **a re-release MUST re-emit
`entity-deployment.json` with `surface`**. Decision: billslab moves to
**`surface: window` (Site Browser)** — the overlay is too confusing/buggy.

**Open bugs — site overlay (why we're leaving it for window mode):**
- The overlay's default home falls back to `demo`@local on a fresh/empty peer →
  `No site manifest at 'demo'`. Sidestepped for window deployments (overlay off);
  still latent for any `surface: site` deployment.
- Overlay routing/back-button: click through to another site and you can't get
  back; navigation "gets lost." Window mode has the directory rail + surface-
  internal cross-site nav, so it doesn't strand you — the reason for the move.
- **To verify:** cross-site links end-to-end in the Site Browser window (mechanism
  is sound — `navigate`/`open_site` `go_to` a new (peer,site) in-window — but no
  e2e phase exercises cross-*site* nav yet).
- **Candidate hardening:** gate the status-bar overlay toggle on `boot_surface`
  (never expose it when booted into a Window) as defense-in-depth, beyond the
  config-emit fix.
- **Testing note:** a returning session's **durable config wins** over the fetched
  one — to see the emit fix, clear storage / fresh profile (else the old
  `show_toggle:true` persists).

## Release-week hardening (2026-07, historical)

The last arc before release was a **mobile / menu hardening pass** driven out of
"get Tauri working." All five fixes shipped and are present:

| Fix | What |
|---|---|
| binaryen 119 pin (`Dockerfile`) | wasm-opt 108 mis-optimized the reference-types funcref table under `-Oz` → `Table.grow` RangeError on **every JavaScriptCore engine** (WebKitGTK/Tauri + Safari/iOS). Firefox/Chrome tolerated it, so e2e never caught it. |
| `demo-apps` Cargo feature (off by default) | Production launchers show the honest empty state; e2e builds `--features demo-apps`. |
| Mobile command palette behind a `☰ Menu` toggle | The menu redesign had made the mobile palette eat the whole viewport. |
| New windows open at TOP of stack (`util::prepend`) | Appended-at-bottom windows scrolled off-screen on autofocus. |
| Games/Apps height floors (`min-height`, not `height:100%`/`vh`) | Percentage/zeroed heights collapse in auto-height tiled `.window` sections — the recurring substrate footgun. |

Stable at the v0.8.0 research-preview line. The two items below are **manual
device-QA sign-offs, not open engineering work** — the code is landed; what
remains is a human observing runtime behavior on a real device (nothing an agent
can execute headless). They were being mis-carried as "release-blockers"; they're
a pre-ship QA checklist.

## Pre-ship QA checklist (human device verification — not agent-doable)

1. **Optimized bundle on Safari/iOS — engineering DONE, device check remains.**
   The fix (binaryen **119** pin, `Dockerfile` — the distro's 108 mis-optimized the
   reference-types funcref table → `Table.grow` RangeError on JavaScriptCore) is in
   source, and `make wasm-release` now rebuilds the optimized bundle **clean** through
   it (verified 2026-07-02). The debug-wasm path skips wasm-opt, so only the release
   path was ever affected — and it now builds green. **Remaining:** deploy the rebuilt
   bundle and open it on a **real iPhone + desktop Safari** (JavaScriptCore runtime
   confirm). No hardware ⇒ agent can't do this; it's a human sign-off.
2. **IndexedDB across-restart durability on WebKitGTK — ✅ VERIFIED (2026-07-02).**
   Confirmed by hand on WebKitGTK/Tauri: `make tauri-run` → created a site → saved →
   relaunched → the site was still there. So the tree persists to IDB across a process
   restart on the Safari-family engine. (Safari/iOS itself still wants the same check on
   a real device, but the WebKitGTK/JavaScriptCore durability question is answered.) The
   persistence path was already sound in source (persist request + honest durability
   banner + OPFS-gap detection); this closes the runtime question.

3. **Two real devices on two real networks — the file-transfer arc's one untested claim.**
   Every P2P gate is one host plus a container rig: `traverse` models two NATs with two
   external addresses and self-hosted STUN, which is a fair model and still a model. What no
   test on this box can produce: a phone on cellular and a laptop on home wifi, a captive
   portal, an ISP CGNAT, a symmetric NAT (which the rig deliberately does **not** model and
   which needs TURN we cannot yet carry credentials for). **The check:** open the SPA on two
   real devices on two real networks, `meet tag <label>` on both, offer a file from one and
   pull it from the other. Report what actually happens — a failure here is data, not a
   regression, and it is the next thing that would change the roadmap.

## Grounding checkpoint (2026-08-17) — read this before continuing the share arc

`CHECKPOINT-2026-08-17-grounding-and-the-cross-stack-convergence-map.md`. Three things in it
change the plan:

1. **There are two applications, not three.** `entity-core-py` has a **very complete handler
   set** (registry, peer-issued registry, discovery + **mDNS**, signaling, network,
   reachability, relay, route, content, local-files, encryption, substitute) and **no app
   tier** — a single-file CLI. So app-tier vocabulary convergence is a **workbench-go**
   conversation; python is the **interop** partner. And that makes python the cheapest route
   to the thing every P2P gate we own lacks: all of them are rust-browser ↔ rust-browser,
   *cohort-consistent, not independent convergence* (ADR-0012). A py peer as the far side of
   an existing transfer gate needs **no app tier at all** — it already has `content`,
   `local_files`, `tree`.
2. **mDNS is implemented in all three kernels**, native-only. A browser cannot speak it — but
   **a Tauri backend peer can**, and hands results over IPC. Ring 1 / LAN ring 2 are therefore
   reachable on the desktop deployment; arch **Q3 narrows** to the plain-web, backend-less,
   listener-less case.
3. **`entity-workbench-go`'s own arc (compute) is dry and waiting on arch**, so a
   sharing/connectivity thread there competes with nothing.

**The arch package is written:**
`ROUTING-2026-08-17-comprehensive-the-content-network-what-we-answer-and-what-blocks-us.md`.
It supersedes the open half of `-16-g`, because reading arch's tree at HEAD changed it:

- **Q1 is WITHDRAWN as posed.** `PROPOSAL-RELAY-COMPLETE-THE-MODE-SET` §2 (arch `95a2f13`,
  opened the same day) already rules the Mode A blocker void — **and names our reasoning as
  defective**: we inferred a substrate capability from an SDK module (`follow.rs`), which is
  §11.1a's own error inverted. The real basis is that cross-peer subscription was always
  permitted by the capability model. Accepted; it is the **fifth** instance this session of
  concluding from an artifact rather than the thing itself, four of them ours.
- **We are named as Mode C's driver** — listener-less, rendezvous-only. Arch's framing is that
  Mode C's deferral, `data_relay` with no credential channel, and an unreachable browser peer
  **are one gap**. We supply the measurements and keep the `turn:` refusal.
- **Our retention finding is already cited** in that proposal's §2.1.2. And **§2.1.3
  (verification through aggregation) is our Follow problem one layer up** — a materialized
  remote subtree in a third party's tree is exactly what we build; `published-root` looks like
  the answer and we asked.
- **Q8 is the new blocker** (`meet` as a DISCOVERY backend), ahead of Q2/Q6.

**Recommended order:** send the arch package → open the workbench-go conversation → python
interop probe → *then* Step 2. Rationale: nothing should touch the share wire shape until
Q2/Q6 return, because with no shares published anywhere yet a namespace/codec change is
**free today** and needs a dated migration read afterwards.

## Backlog

**Quick wins / cleanup**
- `inspect tap` shell verb — ~30 LOC shortcut for `open Path Tap` (last open item in the
  inspect verb set; the other 7 sub-ops shipped).
- Clippy nit at `src/views/shell/binding.rs` (`field_reassign_with_default`).
- `Peers::sdks` Vec compaction — no `detach_worker_sdk`; deleted Backend* peers leave an
  empty SDK slot until reload. Gated on upstream `WorkerProxy::terminate()`.

**Security — deferred, NOT needed for the first release (call made 2026-07-02)**
- **Frame-scoped CSP so the main app can be strict.** Today the app CSP is loose
  (`script-src 'unsafe-inline' 'unsafe-eval'`, Tauri) or absent (browser) because the
  sandboxed `srcdoc` app iframes inherit it and need inline scripts to run. That
  removes a *backstop* against a bug in the site-renderer's HTML sanitizer. **Decision:
  don't do it now** — all shipped content is first-party (we author the sites; apps are
  our entity-apps), the sandbox already isolates apps, and the sanitizer already handles
  site HTML; a CSP on top only guards against our own sanitizer having a hole, and the
  browser already ships with no CSP (no regression). **Trigger to do it:** when users
  routinely browse **untrusted third-party peers' sites** (the open web-of-sites) — a
  malicious page + a sanitizer hole would then run script in the main app with access to
  the user's tree. **Fix:** serve app bundles via a custom Tauri URI scheme (and a
  browser equivalent) that carries its OWN permissive CSP header, so the frame is loose
  while the main app goes strict. (Context: commit `be5573bf`.)

**Performance (ranked, ready)** — see `docs/architecture/reviews/PERF-ANALYSIS.md` §7
- **Entity Tree local-state refactor** — biggest single win; 381–655 `get_entity`/render on
  a 281-row tree, 12.6 ms avg / 25 ms max (over the 16 ms budget). Establishes the
  per-window `HashMap<path,hash>` pattern the others copy.
- KB article-list refactor (mechanical copy of that pattern).
- Event Log + Query Console shared `CachedEventLog` ring buffer.

**Shell extraction (Phases 4–6)**
- Tier-E verbs still open: `revision`, `history`, `role` (SDK ops landed; verbs unwritten).
  `identity`, `compute`, `inspect` (7 sub-ops) done.
- Phase 5: standalone `entity-shell` binary + one-shot `dls`/`dcat`/`dexec` (now ours to
  land under bindings/shell ownership).
- Phase 6: persistence-helper consolidation in `app_paths.rs` vs crate helpers.

**Open product decisions (need sign-off, not unilateral)**
- Stage-A2 tree search — half-wired (`set_search` exists, `flatten_visible` ignores it);
  cross-impl shared-shape question with workbench-go.
- `src/action_event.rs` keep/delete — zero callers *by design* (cross-impl schema anchor).
- Query/count/execute primary hard-code in `app.rs` — latent peer-scoping bug; fix = thread
  the window's `peer_id` through `Action::Query`/`Count`/`Execute`.

**Persistence**
- Offline-wipe bug: hard-refresh while server unreachable wipes local state; needs a
  hash/version handshake + offline-keeps-local design.

**Long-deferred capability stages** (from `SYSTEM-VISION.md`): KB wiki PoC, type renderer
registry, pipeline builder (SDK Layer 2), relay (Tauri backend), capability + identity arc
(Key Manager stays placeholder until then), cross-renderer portability, self-modification.
Pull into roadmap when scoped.

**Build & release follow-ups**
- `Cargo.lock` is gitignored — commit it for reproducible release builds.
- `dist/` hygiene — ship `make wasm-release` (default features), never a debug/`demo-apps`
  `dist/`.
- F-1: vault label `|`/newline not escaped (`vault_codec`); do in a calm window with input
  validation.

## Waiting on

- `entity-core-rust` (required sibling) — this crate uses path dependencies to
  `../entity-core-rust/`; build fails at dependency resolution if that checkout is missing
  or at an incompatible revision. SDK-tier changes belong upstream there, not here.

## Done recently

- **Publisher identity + first-paint discovery + startup-surface arc (2026-07-02
  session — see `HANDOFF-2026-07-02.md`).** **Durable publisher identity**
  (`persistence::publisher_keypair`) is now the publish default — no more shared demo
  `2KEB3…` id; the containerized `--rm` publish persists it via a gitignored
  `PUBLISH_DATA_DIR`/`ENTITY_DATA_DIR`, shared with `publish-serve`. **Boot-time
  site-discovery warm-up** (`discovery::warm_peer_sites`) fetches each registered
  peer's `sites.list`+manifests and caches them, so published sites show on **first
  paint** (not after a navigate). The **"show my site" posture boots a maximized Site
  Browser window** (not the fragile overlay); `strict-site` keeps the overlay/kiosk.
  Startup flow documented (precedence + the 3 boot surfaces). Two e2e fixes (QR is
  display-gated to a real listener → hidden in the browser; deployment fixture pins
  strict-site). **Live decision handed to next session: kill the `profile` presets,
  expose the real startup settings (surface / window_type / escapable) directly.**
  9 commits on `dev`; **e2e 12/12, 608 native tests, wasm + clippy clean.**
- **Content-site + publish-pipeline arc (2026-07-01 session — see
  `HANDOFF-2026-07-01.md`).** Markdown rendering fidelity (GitHub-like tables/code/
  blockquotes on both the in-app `.cs-doc` and the static export) + a full "Markdown
  Showcase" demo page; the bundled demo re-authored as an on-disk **ingest-format
  example** (`examples/entity-demo/` — canonical domain→sites layout, dogfooded through
  `make publish --ingest`); **publish-pipeline cutover** (ripped the papers-specific
  `publish-papers`/`PAPERS_*` wiring incl. the `[internal]` leak-path, parameterized apps
  onto `APPS_DIST`, generalized `publish-serve`); static-export now **emits asset files +
  rewrites `<img src>`** (static-site images resolve); `publish-serve` **emits
  deployment-config by default** so the served SPA lists published sites; **service-worker
  cache fix** (unconditional shell fetch + `updateViaCache:'none'`) so a reload picks up a
  new build instead of a stale 304. 7 commits on `dev`; 606 tests green, wasm clean.
- Release-week mobile/menu hardening — the five fixes above, all shipped.

## Next

**Connectivity thread (current):**

1. ~~**P0 — `reflection_endpoints` in `entity-core-rust`'s signaling advertisement**~~
   **CLOSED 2026-08-15/16.** All three implementations carry it (rust `0b4e0cd`, bytes pinned
   against the Go node); a connector now *learns* what its node serves, and
   `make e2e-webrtc-advertised` is the gate — the node publishes a reflector and the browsers
   type nothing. Two things that cost a red gate and are worth keeping: the learning has to ride
   a path the user already walks (recording only on the Check button is the automatic half
   wearing a manual hat), and typed vs advertised are separate fields merged at read.

2. **`entity-workbench-go` app tier.** Its kernel is ready — core-go has `ext/signaling`
   (punch, pool, coordinator, node), the §10.3 seam with single-flight, and srflx — but the app
   tier has none of it (no `MaintainPeer`, `peer/status`, `connector`, or `meet`). It needs the
   same four pieces we built here: liveness read-model (its `ConnectedPeers()` is a pool
   snapshot — the `connection_health` shape we deleted), a `maintain-peer` driver,
   transport-profile publish, connector registry + meet. **No WebRTC needed** — Go has no stack
   and does not need one to interoperate over WebSocket. **The prize:** every P2P gate we have
   is rust-browser ↔ rust-browser, which is *cohort-consistent, not independent convergence*
   ([ADR-0012]). This buys the first genuinely independent evidence for the product surface.
3. **TURN** — `parse_ice_urls` refuses `turn:` because there is nowhere to put a username and
   credential. Symmetric NAT, which the traverse rig deliberately does **not** model, needs it.
   The next real reachability increment after (1).
4. ~~**Awaiting an arch ruling:** §10.3 obligation 6 (the consultation bound)~~ **RULED —
   `NETWORK` v1.7** (arch `fe935be`, routed `ROUTING-2026-08-16-g-the-seam-consultation-bound-is-ruled`).
   Closed on all three impls; go's dispatch bound green, rust confirmed charge-at-start, py
   reports the seam binds vacuously (no §10.3 site). Our proposal
   (`entity-core-rust/docs/PROPOSAL-ESTABLISH-CONSULTATION-BACKOFF.md`) is spent. *This line sat
   stale for a day — caught in the 2026-08-17 grounding audit.*

**File-transfer thread (new, 2026-08-16):**

- **Two real devices on two real networks** — the arc's one untested claim, and the only item
  here an agent cannot do. Details in the pre-ship QA checklist above. Nothing further on this
  box shrinks it.
- **Capability management for the serving side.** The browser is a resource holder now and the
  enforcement plan has no row for it: no grant profile covers the offer path, and a browser peer
  cannot author policy on its own tree, so an enforced deployment loses browser↔browser transfer
  entirely. Sharing is also all-or-nothing — *"share this file with THIS peer"* is not
  expressible. Recorded as §9 of `PLAN-OF-RECORD-capability-enforcement.md` (which is where the
  cutover decision lives) and buildout item 22. **Deliberate:** the arc's goal was to prove the
  technology; refinement follows.
- **Retention / streaming ingest.** An offer is permanent (nothing can reclaim ingested content
  — buildout item 21, routed) and one file is capped at 16 MiB by an offer-side memory bound.
  Both are the same piece of work: ingest in batches the way the pull already fetches, and give
  "unshare" something to actually delete.

**Network / content-sharing thread (new, 2026-08-16) — the layer above connectivity:**

Design of record: **`reviews/DESIGN-THE-CONTENT-NETWORK-AND-THE-USER-MODEL.md`**. The audit
behind it found three partial naming systems and **no** use of `EXTENSION-REGISTRY` (which is
implemented upstream and in `entity-peer`'s default features — we opt out), and **four
hand-rolled implementations of one idea** (chat follow+poll, offer closure walk, site HTTP
poll, boot warm-up) while `entity-sdk`'s `follow` packages exactly that and has one consumer
on one arm. The missing pieces are two nouns — a **Share** (a thing published, with an
audience) and a **Follow** (a prefix mirrored, with a budget) — and the missing user model is
four rings: this device / my devices / my people & groups / the open network. Ring 1 (my
devices) does not exist at all.

- **Initial design landed: `reviews/DESIGN-SHARE-FOLLOW-AND-THE-GRANT-AS-INTERFACE.md`.** The
  alignment claim holds and the interface already existed: `GrantEntry{handlers, operations,
  resources, peers}` — four axes, and the extension roster / peer management / the tree /
  a share are four projections of it. So **a Share is a titled grant, not a second permission
  system**. Convergence targets from `entity-workbench-go` (`mount`/`mounts`, `revision follow`
  with an explicit bootstrap, a capability minted per `(remote, prefix)`, config in a *system*
  namespace) — with one deliberate divergence: their Form 1 (`base=$notification.previous_hash`)
  needs reliable delivery, and our WebRTC path is not that, so the browser must be Form-2-shaped.
- **Three carried claims corrected, by compile check.** `entity-capability-handler`,
  `entity-registry` and `entity-discovery` **all build clean for `wasm32`**, all are registered
  behind feature gates in `core/peer/src/lib.rs`, and **none is in our main `entity-peer` dep**
  (capability-handler is dev-dependencies-only). Enabling them does **not** flip enforcement —
  `debug_open_grants` is a separate `PeerConfig` field. So buildout item 22's mechanism was
  wrong: not structural, one Cargo line. Step 0 of the plan.
- **Step 0 LANDED (`95c18dc`) — `system/capability` + `system/registry` are now registered on
  the browser peer**, both arms (feature unification reaches the `entity-worker` bin; verified
  in the built artifact, not inferred). It also closed a latent defect:
  `default_connection_grants()` advertises `system/capability:request` unconditionally and its
  own doc requires an advertised grant to name only registered handlers — we advertised it with
  the handler absent, invisible only because `debug_open_grants` replaces those grants. It would
  have surfaced exactly at the enforcement flip. Cost, measured: **release +221 KB per bundle**
  (+3.32% browser, +6.17% worker). `discovery` deliberately left off — mDNS is its only v1
  backend and a browser cannot speak it.
- **Step 1a LANDED (`abc00ac`) — `src/share.rs`.** `Share::grant()` derives the 4-tuple; there
  is no `is_visible_to` predicate anywhere, deliberately (that is how a second permission system
  arrives by the back door). Nine tests; `a_prefix_share_does_not_authorize_a_sibling_prefix` is
  **mutation-checked** (dropping the trailing-slash normalization → red on exactly that assert).
  A `FileOffer` round-trips through a `Share` with the same id; a site share has no file-offer
  form. **Namespace stayed app-tier** — inventing a `system/` convention unilaterally is what
  AGENTS-STANDARD forbids, so it waits on Q2 behind one path helper. **No `Group` audience** —
  Q7 decides whether leaving a group revokes access, and expansion would silently mean "whoever
  was a member that day". This slice derives and encodes only: it does **not** author onto the
  policy table, and no surface may read an audience as a control while `debug_open_grants` holds.
  **Gates at this HEAD, all unfiltered:** `make test` **1035/0** (was 1026 — nine new), `make
  lint` clean, `make wasm` + check-dist consistent, `make e2e-worker` **17/17 in 296.97s**
  (baseline 296.46s). The webrtc gates were not re-run: `share.rs` has no callers yet, so
  nothing it contains can reach a transport — re-run them at Step 1b, which does.
- **Step 1b LANDED (`a104b7a`) — a share authors a grant on our OWN tree**, the half buildout
  item 22 says is missing. The finding that shaped it, from reading `handle_configure` rather
  than assuming: `configure` **replaces** the entry for a `peer_pattern`, so per-share authoring
  is last-write-wins — sharing a second file with the same audience would silently revoke the
  first. The entry is therefore the **union** of every share filed under that key, recomputed on
  publish *and* withdrawal (mutation-checked). Withdrawal writes an **empty** grants array —
  that is what revocation consists of, and omitting the write leaves a stale grant (the
  `apply_offers` lesson again). Also separated two vocabularies that look identical: `peers: ["*"]`
  inside a grant vs the policy path segment, where `*` is **illegal** and public files under
  `default`.
- **Step 1c LANDED — and the revert improved the design.** The first attempt fused *publishing*
  (write a manifest) with *authoring* (compute a policy union from the complete set), and had to
  be reverted because the union's precondition — a fully-seeded Worker mirror — cannot be met at
  write time. Separating them removed the precondition from the write path altogether:
  `publish_share`/`withdraw_share` need no `&Peers` and no share set (so the pending-queue the
  first attempt required is **gone**), and `ShareSync` — an app-lifetime watch on the shares
  prefix, the `user_themes` precedent — re-authors the policy whenever that prefix is dirty,
  converging as the mirror seeds instead of racing it. Offering a file now publishes a
  `kind: file` share; withdrawing drops it and the reconcile writes the empty entry that revokes.
  **The general rule worth keeping: when a write needs a complete read to be correct, don't do
  the read at the write — dirty something and let a reconcile converge.**
- **Design flexibility is now audited, not assumed** — `DESIGN-SHARE-FOLLOW…` **§6b** is a
  reversibility map: for each open arch question, exactly which functions change and whether it
  is additive or breaking. Every share-path construction site is inside `share.rs` behind two
  `app_paths` helpers (verified by grep), so Q2's namespace move is 2 fns + 1 const. **The one
  window that closes:** with no shares published anywhere yet, a namespace/codec change is free
  today and needs a dated migration read once users publish — so if a Q2/Q6 ruling is close, it
  is cheaper to wait than to migrate. Also listed: what would *not* be cheap (a `Group` audience
  shipped by expansion; enforcement flipped before Q6/Q7; any second permission system).
- **A → E in §7 of that doc.** Near-term: generalize `offers/` → `shares/` with a `kind`;
  make a **site a share pulled over the peer connection** (its mirror destination
  `site-cache/{peer}/sites/{id}/` already exists and the resolver already reads it) with a new
  `make e2e-webrtc-site` gate; a first Network window. `Follow` as a real record is the deeper
  one and pays off on every later app.
- **Routed to arch:** `ROUTING-2026-08-16-g` — five questions, of which Q1 is the big one
  (**is `REGISTRY` §8.2 aggregator federation still blocked on RELAY Mode A**, given that
  `follow`-continuation materializes a remote prefix into a local tree and is proven
  cross-peer?) and Q2 is the one with an interop deadline (who owns "what is this peer
  offering" — `published-root` is singular and audience-free).
- **Do not** retire the chat poll, build a DHT/gossip, or ship enforcement expecting shares to
  work. Retention (buildout item 21) must be **sequenced with** prefix sharing, not after it —
  sharing a site ingests every page and nothing reclaims it.

**Product thread (carried, unchanged):**

5. **Republish / incremental-publish analysis** (operator flagged for its own session):
   republish the same site, add one app/article, immutable content store vs tree
   rebuild, orphan pruning, peer-id churn. Deliver a documented, coherent republish
   pathway before a wide release. (Details in the handoff.)
6. **Locked-surface safety** (longer-term, per handoff): don't let users self-lockout —
   confirm + temporary password on entering a locked mode, a "lock the settings surface"
   option, and a documented recovery path (`?chrome=1` / `?systemrecovery=1`). Design
   end-to-end before shipping locked Window/kiosk modes.
7. **Pre-ship QA sign-offs** (human device checks — see the checklist above, NOT
   engineering-open): the rebuilt `make wasm-release` bundle on a real iPhone + desktop
   Safari; IndexedDB across-restart durability under WebKitGTK/Tauri (`make tauri-run`,
   runnable locally). The Safari fix (binaryen 119) is landed + the release bundle
   rebuilds clean; only the on-device observation remains.
8. **Entity Tree perf refactor** — self-contained, over budget today; establishes the
   per-window local-state pattern the other views reuse.

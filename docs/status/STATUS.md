# entity-browser-rust — status

_Updated: 2026-08-15 · public: v0.8.0 (master) · working branch `dev`_

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
alone is enough. Re-verified at HEAD 2026-08-15 alongside `make test` **1001/0**, lint, wasm.

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

1. **P0 — `reflection_endpoints` in `entity-core-rust`'s signaling advertisement**
   (`extensions/signaling/src/{core,data}.rs`). The **only** item on our critical path. It is
   the *automatic* half of ICE provisioning: a node telling browsers which STUN to use, which
   fills in `Connector.ice`. Routed to us as P0 by arch's cohort packet
   (`ROUTING-2026-08-15-cohort-packet-service-advertisement-and-reflection-endpoints`) with
   `entity-browser-rust` named as the blocked consumer — we consume rust's client decoder
   directly, so the field must exist there before it can exist for us. **One optional field,
   both surfaces** (§2.2 makes the unwrapped §9.2 response a MUST too, and it is the easy half
   to skip). The `endpoint`-as-URI-string fork that blocked this is **settled** — arch ruled it
   2026-08-15 (`EXTENSION-REGISTRY` §3b.0, RFC 7064 `stun:`), so this is no longer upstream-blocked.
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
4. **Awaiting an arch ruling, blocking nothing:** §10.3 obligation 6 (the consultation bound) —
   proposal at `entity-core-rust/docs/PROPOSAL-ESTABLISH-CONSULTATION-BACKOFF.md`, routed
   `ROUTING-2026-08-15-b-item-6-is-remedied-and-three-things-we-told-you-were-wrong`.

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

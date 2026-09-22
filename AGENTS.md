# entity-browser-rust (DOM/WASM) — AGENTS.md

Read **AGENTS-STANDARD.md** first. This file adds entity-browser-rust specifics.

## Overview

DOM-primary Rust/WASM application for entity-core: a window-manager / UI shell
over the entity tree, rendered as HTML DOM in the browser and in a **Tauri**
desktop WebView. Cargo package is `entity-browser-rust`; the on-disk dir was
renamed from `egui-entity-core-rust` to match. HTML DOM is the
**only** render path — the legacy native/egui renderer is gone, `EntityApp` is
wasm-only, and plain `cargo build` produces a deprecation stub.

## How we work here — Disciplines & Doctrines · tier **FULL**

This repo runs the entity-OS **Disciplines & Doctrines** methodology at the **Full** tier —
how a complex, non-deterministic runtime (the browser/WASM substrate, far from the spec) is
held where conformance alone can't. The framework is `METHODOLOGY.md` (injected, identical
everywhere); this repo's charter below carries the local grounding, and **it is one of the two
worked instances the framework was reconciled from** — D1–D11 there are the universal set,
D12–D16 here are ours, earned on our own bugs.
- **Disciplines** (invariants — the *what*):
  `docs/architecture/specs/DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md` —
  D1–D16 and D19–D21 ratified, plus **D23** (no unbounded network await on the boot
  path — ratified 2026-08-27 on a reproduced run, with all three enforcement points)
  **D24** (any durable copy of someone else's bytes is a cache and needs a currency
  trigger — ratified 2026-08-29 on a gate observed red, with both enforcement points)
  and **D25** (a refreshable value carries who set it; a heuristic on the value is not
  provenance — ratified 2026-09-02 on two instances in different subsystems, with both
  enforcement points, and **on the ladder's early-promotion terms**: reviewed against a
  third instance found by someone else, removed if unearned within a release cycle);
  candidates at D17, D18, D22. The per-diff review questions (nine + 5b), anti-pattern
  catalog AP1–AP53.
- **Doctrines** (Feature/Audit procedures — the *how*):
  `docs/architecture/specs/DOCTRINES-BROWSER-SUBSTRATE.md` — open the Feature
  Development Doctrine (F0–F8) for "build X", the Audit Doctrine (A0–A12) for "Y is broken".
- **Substrate model** (ground truth — read before any leak / freeze / lifetime / persistence
  work): `docs/architecture/specs/MODEL-BROWSER-WASM-RUNTIME.md`.
- **Stakeholder / ownership model** (ground truth — read before any rule about *who owns a piece
  of state*, what may be refreshed from a remote source, what a cutover or rollback may replace,
  or what a deployment decides on a visitor's behalf):
  `docs/architecture/specs/MODEL-STAKEHOLDERS-AND-OWNERSHIP.md`. Three **positional** roles
  (vendor / deployer / end user — one party often holds all three, which is how they collapse in
  analysis); **two** partitions, only one of which exists; and the fact that the
  deployer↔end-user line is **configured per deployment**, so an ownership table is the default
  when a deployment says nothing, never the model. Four separate design threads each drew this
  line differently before it was written down.
- **The SIGNALING/NETWORK buildout** (this repo as the **browser leg** of `ROADMAP-EXTENSIONS`
  Stage B — every connectivity change we made, tracked to the spec section it answers, plus
  the open items and the gates not yet run):
  `docs/architecture/reviews/BUILDOUT-SIGNALING-AND-NETWORK-EXTENSIONS.md`. Read it before
  touching signaling, `§10.3`, transport profiles, or liveness — and **add to it** when a
  connectivity change produces a spec finding, so the finding does not end up as folklore in
  a handoff. Spec sections are cited, not restated; arch wins on all of them.

Session start: read the charter → the model. Task start: open the matching doctrine. Every
feature/audit ends by feeding its lessons back into the disciplines — the ratchet (*a feature
must make us stronger, not weaker*).

## Setup / environment

- **Rust only** — `cargo` + Tauri; **no Node/npm** anywhere in the toolchain.
- WASM toolchain (trunk-style build into `dist/`) for the browser/WebView target;
  `src-tauri/` is the native desktop backend (Rust over IPC).
- Build is `make` over **podman** (see AGENTS-STANDARD). `make e2e-worker` needs
  a Selenium container (headless Firefox) on `:4444` — **`make e2e-grid` stands up a
  correctly-configured fresh one** (2 GB `/dev/shm`; podman's default silently
  destabilises Firefox). Details in `tools/e2e/README.md`.
- **Host needs only `make` + `podman`** — every target MUST run in the image
  (`serve`/`build-serve` serve via a containerized `python3` over `--network host`;
  rootless `-p` port-publish is unreliable under pasta/slirp, so serve uses
  host-net). `site-serve` is now containerized too: the `cargo run … publish`
  runs via `$(call RUN,…)` and the serve via `$(call RUN_SERVE,…)`, both with
  `$(SERVE_DIR)` bind-mounted (the /tmp isolation + safety guard intact) and
  `ENTITY_DATA_DIR` translated to its in-container path — so all three publish
  modes share ONE durable identity. Verified end-to-end on a podman-only host
  (SPA + `/sites/` + `/content/` + `entity-deployment.json` all served 200).
  - **`make tauri-run` / `tauri-bundle-run`** are **also containerized** now —
    the desktop GUI launches from inside the image (webkit2gtk is IN the image;
    the build already links it — NO host webkit needed) onto the host's display
    via the `RUN_GUI` macro. Wayland-primary (falls back to X11/XWayland). Three
    load-bearing flags: `--security-opt label=disable` (Fedora SELinux otherwise
    blocks the compositor socket — it shows `-?????`), `--userns=keep-id` (socket
    uid match), and `WEBKIT_DISABLE_DMABUF_RENDERER=1` (WebKitGTK's GPU dmabuf
    path is unreliable in a container → force software compositing). The only
    real requirement is **a display on the host** (Wayland or X11) — a truly
    headless box has no window to present, but that is the *sole* limit; make +
    podman is still it. `rm -rf .tauri-home` (gitignored per-app HOME) for a
    fresh cold-boot profile. **Verified booting on this Wayland host.**
    - **File-transfer share is host-visible, not container-trapped.** The
      backend's `local/files/shared` root is `$HOME/.entity/tori-share`
      (`ensure_share_root`, src-tauri); since the container's HOME is the
      `.tauri-home` bind-mount, shared files live on the host at
      `.tauri-home/.entity/tori-share/` and persist. To share an **arbitrary
      host folder** instead: `make tauri-run SHARE_DIR=~/Downloads` bind-mounts
      it onto the share root (transfers land straight in that host dir).
  - **`make host-run`** is the **build-in-container, run-native** alternative —
    the container-built binary run as a plain host process, so NONE of the
    passthrough/SELinux/SHARE_DIR plumbing applies: your real `$HOME`, real
    filesystem (share = your actual `~/.entity/tori-share`), native display.
    Build stays containerized; the one host runtime dep is the WebKitGTK lib the
    binary links (`sudo dnf install webkit2gtk4.1` on Fedora — gtk3/libsoup3 are
    already present). A GUI runtime lib, not a build toolchain. Pick `tauri-run`
    for zero host packages, `host-run` for zero runtime plumbing.

## Build & test

```bash
make test          # native unit/integration suite, all test binaries
make test-tauri    # the src-tauri backend suite (workspace-excluded from `make test`)
make lint          # Clippy + the UI/i18n/boot-fetch/tree-hygiene linters
make wasm          # WASM debug → dist/   ← MANDATORY after every change
make wasm-release  # WASM release → dist/ (opt-level=z + LTO + wasm-opt -Oz)
make serve         # serve dist/ on :8081 (plain browser, no Tauri). Every serve target
                   # goes through `tools/cors-serve.py`, never `python3 -m http.server`
make build-serve   # build OPTIMIZED release THEN serve :8081 (serve does NOT rebuild)
make pair-serve    # build-serve with THIS machine's signaling node BAKED IN — a browser
                   # that merely loads http://<this-ip>:8081 is already paired
make tauri-run     # build WASM + Tauri, launch with stdout logs (active desktop path)
make host-run      # build in the container, run the binary natively (needs webkit2gtk)
make e2e-grid      # FRESH Selenium node, correctly configured (shm 2g). Run this FIRST.
make e2e-worker    # Worker-mode E2E, headless Firefox; Selenium :4444, serves :8092
make e2e-phases    # list the e2e test names + phase labels the filters accept
make noscript-check # the apex as a NO-JS agent sees it (crawler/text browser). Needs the
                   # same Selenium grid — do NOT run it beside e2e-worker
make e2e-webrtc-chat       # two browsers chat over §6.5 WebRTC — the MECHANISM
make e2e-webrtc-meet       # two browsers meet at a name then chat — the SHIPPED PATH
make e2e-webrtc-meet-noreload  # …the same journey with NO reload — the late-arm gate
make e2e-webrtc-advertised # the node PUBLISHES its reflector (§4.5.1); browsers type nothing
make e2e-webrtc-lan        # SAME LAN as a real browser does it — mDNS `.local`, 0 reflectors
make e2e-webrtc-idle       # survives-idle across two NATs — exits 0/1/**2=INCONCLUSIVE**
make e2e-webrtc-nat        # NEGATIVE control: rendezvous works, media must NOT cross
make e2e-webrtc-file       # two browsers meet at a name, then one SERVES a file to the other
make e2e-webrtc-file-crossengine  # the SAME transfer across TWO ENGINES (firefox↔chrome, 6.5 MB)
make e2e-federation        # publisher, app origin and browser as three separate hosts
make crossimpl-go          # OUR reader vs core-go's LIVE publisher, two hosts
make dist          # RELEASE installers for THIS host → artifacts/ (ADR-0023 Mode 1)
make dist-web      # the browser SPA as a release tarball → artifacts/
make dist-native   # = make dist NATIVE=1 (host toolchain instead of the container)
make dist DIST_OS=windows XWIN_ACCEPT_LICENSE=1   # Windows installer, FROM LINUX
make site          # emit content sites (this was `make publish`; `publish` is reserved)
make site-dist     # the UPLOADABLE production web tree → dist-site/ (SPA + content + apps
                   # + deployment config). `make site` alone is the CONTENT HALF ONLY.
                   # REFUSES on a dirty tree (either half of the pair) before it builds:
                   # CORE_RUST_REF=<ref> to pin the kernel, ALLOW_DIRTY=1 to waive dirt
                   # (it does NOT waive a pin mismatch — that is a different failure).
make build-pair    # which two commits is this build made of? Host-side, no image.
make registry BIND='--bind=NAME=PEER_ID@ORIGIN'   # publish a NAME REGISTRY + verify it
make federation    # the whole chain locally: N domains + a registry → dist-federation/
make native        # DEPRECATED — prints redirect, no native UI build
```

**Do not quote a test count from this file.** It goes stale in hours, the binary count
moves too, and "the main binary" and "the full suite" are different denominators that
session notes have quoted interchangeably. Re-measure, and say what you measured —
**and with what invocation.** `make e2e-worker` unfiltered scores differently with and
without a display on this host (the Tauri WebView phase is a standing red here, bisected
2026-09-03), so a bare *"unfiltered, 64/0"* is not reproducible and cost a session a
false regression. The headless spelling is `env -u WAYLAND_DISPLAY -u DISPLAY make e2e-worker`.

- **Always run `make wasm` after changes.** Native tests run against the Direct arm and
  the native target; they cannot catch WASM/Worker-only breakage (cfg-gated code, missing
  imports, type inference, arm-split panics).
- **`make test` compiles the e2e suite to NOTHING** — `tests/e2e_worker.rs` is
  `#![cfg(feature = "e2e")]`, so a green `make test` is no evidence that file even parses.
- **Any network read on the boot path is bounded — D23, and it has three enforcement points.**
  `net::fetch_text_bounded` (`src/net.rs`) is the Rust chokepoint and `fetchWithDeadline` the
  `sw.js` one; `tools/net-lint.sh` (in `make lint`, baseline-ratcheted) gates new raw fetches;
  the behavioural gate is the black-hole pair below. **The rule bounds an await blocking a
  DEFINED ALTERNATIVE outcome, not every `fetch`** — two `sw.js` fetches are deliberately
  unbounded. Read the GOTCHAS entry before "fixing" one.
  **The baseline carries a COUNT, not names, and this file said "by name" until 2026-09-02.**
  `assets/sw.js sw_raw_fetch=3` is the whole record: the two deliberate ones **plus
  `fetchWithDeadline` itself**, which is a bare `fetch` by construction (the same reason
  `index.html`'s BIOS helper holds the one bare fetch there). **The gap that wording hid: a SWAP
  passes.** Delete a deliberate fetch, add an accidental unbounded one, and the count is still 3
  and `make lint` stays green — so the ratchet catches *more* raw fetches and cannot catch a
  *different* one. D23's lint is a floor on quantity, not an allowlist of sites; when you change
  any of the three, re-read them rather than trusting the number.
- **ANY copy of someone else's bytes is a cache and needs a currency trigger; `if absent` is not
  one — D24. Go through `content_site::foreign_cache::ensure_current`; do not call
  `http_poll::fetch_*` yourself** (`tools/foreign-cache-lint.sh` in `make lint` will stop you,
  baseline-ratcheted). The durable entity tree is the single source of truth *for state we own*;
  the moment you write a foreign artifact into it under `/{me}/{foreign}/…` it is **a cache**.
  `sw.js`, the browser HTTP cache (`Freshness::Mutable` → `no-store`) and the registry TTL all
  get this right — and a presence check in the store **short-circuits all three, because no
  request is issued at all**. **We never had to invent change detection: hop 1 of the two-hop is
  a 58-byte pointer that changes iff the entity changed, and the local half is already in the
  tree** (an `Entity` carries its canonical `content_hash`) — which is also why
  `CacheProvenance::pinned_root_hash` was a duplicate of a fact we already had, not a missing
  instrument. Two corollaries that are where this goes wrong: **an unreachable origin leaves the
  held copy untouched** (a cache that drops what it cannot re-verify turns an outage into a
  missing app), and **`Unchanged` writes nothing and flips nothing dirty** — on the Apps surface
  a spurious dirty restarts a running app. Background:
  `docs/architecture/reviews/AUDIT-2026-08-28-CACHE-FRESHNESS-EVERY-COPY-OF-SOMEONE-ELSES-BYTES.md`.
- **`make e2e-worker T=an_app_republished` is D24's behavioural gate — publish, boot, republish
  under the SAME identity, boot again, assert the new bytes are on screen.** It exists because
  **no gate here had ever visited an origin twice across a publish**, which is exactly why
  devops found a wedged app within minutes of a real deploy and a green suite never could. Run
  it for any change to the fetch layer, `foreign_cache`, the Apps window's fetch path, or either
  site-manifest sweep. Its fixture pair (`emit_app_republish_v1`/`_v2`) reproduces the
  production shape: **byte-identical catalogs, only the bundle pointer moves.** Assert the
  rendered marker, never that a fetch happened (AP31) — and note it runs on the Direct arm,
  because on the Worker arm the presence read answers from an asynchronously-filled mirror and
  can go green for the wrong reason.
- **`make e2e-worker T=a_new_build_reaches` is THE HOTFIX GATE — release-risk R-2's live half,
  and the property the whole deploy story rests on.** *We will ship bugs, so a fix must be picked
  up on the next refresh and heal the profile.* If a service worker can pin a broken shell, a bad
  deploy is not an incident, it is a **brick** (matrix cell #10, recoverable only via C17's
  self-destruct worker — **built and drilled 2026-09-02**, and still the expensive path: it costs
  every visitor their offline shell, so this gate staying green is what keeps you off it). It
  stages an isolated SPA copy, boots it, **asserts a service worker is CONTROLLING the page**,
  moves the shell's `entity-build` stamp, reloads once, and asserts the app reports the new build.
  Run it for any change to `assets/sw.js`, `index.html`'s registration block, or the cache
  headers — **and run `T=the_kill_switch` beside it**, since the two are the same subsystem's
  ordinary path and its last resort.
  **It existed as a sentence, not a gate, until 2026-09-01** — the deploy transition was recorded
  in `the_worker_bundle_is_fetched_once_per_build_not_once_per_load`'s doc comment as
  *"mutation-checked in both directions"*, i.e. a manual check months ago, on the path we can
  least afford to be wrong about (AP37).
  **The anti-vacuity half is the design, not decoration:** a reload picking up a new document
  proves nothing if no worker was in the path — that is plain HTTP wearing R-2's name. Falsified
  three ways: a `networkFirst` that serves its cached copy first reds with *"THE HOTFIX DID NOT
  LAND"*; a page with no registration reds **VACUOUS**; and the positive passes. **Scope, stated:
  it moves the shell, not the hashed bundle** — the pinning risk is entirely the mutable shell,
  since `index.html` names every hashed asset and those are `cacheFirst` by content-addressed
  URL, where a changed URL *is* the invalidation.
- **A durable record of a REMOTE assertion carries the path back to that assertion — AP30.**
  Anything written down because a deployment doc, registry or peer said so must be re-checked
  whenever the source is in hand, and dropped when the source contradicts it; a write path
  whose trigger fires exactly once never re-examines its own premise, so an ordinary mistake
  at the source becomes permanent client state (`peer_supersession::revalidate`, gated by
  `a_supersession_the_domain_contradicts_is_dropped`). Two rules the naive version gets wrong:
  **no source this boot changes nothing** (a truncated doc must not be able to wipe good
  state — and D23's deadline makes that case *more* common), and **revalidate strictly after
  adopting**, or a legitimate second divergence reads as a stale record. And the corollary that
  looks like an optimization: **an origin's "I have none" is a fact you may REPORT, never one
  you may write down.** `deployment_config::read_document` is **five**-state (`Served |
  NoDocument{status} | OriginError{status} | Unreadable{status} | Unheard`) so the log can tell a
  domain that ships no config on purpose from one nobody could reach, **and both of those from a
  domain that answered with a fault** — `OriginError` was split out later and this line said
  "four" until 2026-09-01, which is the stale-doc shape AP40 warns about one layer up: a 502 is
  not a deployer choosing to serve no config. Caching that 404 to skip the next probe would be
  AP30 with a shorter fuse: a deployment that *adds* the document later would never reach a
  returning profile. Only `origin_answered()` may be branched on.
- **When you SPLIT a collapsed value, the default arm gets the WEAKEST claim — AP40.** Twice in
  one session a change that existed to stop conflating outcomes handed its most specific
  statement to the `_ =>` arm: every non-2xx became *"this deployment serves no config **by
  choice**"* (a 502 reporting the deployer's intent), and every answered-404 became *"the
  publisher withdrew this site"* (regressing the wrong-host case the message it replaced was
  earned on). **Enumerate what reaches each arm and ask what it licenses you to say; prefer an
  evidence test over a status test.** Then check the test doubles, because a stub that
  misreports which failure it produces makes the new distinction untestable (AP39) — the sweep
  of every `impl BinSource` is on record.
- **A sync read at construction reads nothing on the Worker arm and RACES the store on the
  Direct one — and keeping the result is the bug, not the read (AP41).** `Peers::get_entity`/`tree_listing` answer from the
  per-prefix cache mirror. Every window factory here reads (`model.initialize(pm)`) *before*
  it subscribes, and **reordering does not fix it**: `observe` is async, so a subscription
  makes the *next* read work and a constructor that caches has no next read. This is not the
  "subscribe the prefix you read" rule — that one is about coverage; this is about
  **retention**. Render from a cold read if you like; do not retain it. Use
  `get_entity_async`/`tree_listing_async` (L1, subscription-independent on both arms) — the
  rule `boot_load` already applies to the session config (*"the durable tree, not the cold
  cache mirror"*) and `tree_listing_async`'s own doc comment states for listings. Two traps in
  the repair: **an errored round-trip is not an answer** (keep what you have — AP30 corollary
  (a)), and **a write that lands during it is newer than it** (guard with a generation counter,
  or you drag the user backwards). Report the outcome as an enum — *restored / never had one /
  could not tell / you moved first* are four facts (AP40). Measured 2026-08-30: every
  returning reader on `?worker=1` was put back on the **build** default and shown *"No site
  manifest at 'demo'"* — and **the shipped Direct-IDB arm has the same defect intermittently**
  (the store fills from IndexedDB while `initialize` runs; measured as a 1-in-3 flake, and
  neutering the fix reds BOTH arms). Do not file this class as Worker-only.
  **The class has an enforcement point now: `WindowView::hydrate_durable`, a defaulted trait
  method `WindowManager::spawn` calls for EVERY window.** Override it; do not add a call to
  your factory (that was the original shape and it made the step something the next author had
  to remember — AP44). **All eight models are fixed** as of 2026-08-31; the six that came last
  go through `crate::window_hydration::durable_hydration_job`, which owns the three traps and
  leaves each surface only its own adopt. **Write a new window? You do not get to forget:**
  `tests/window_hydration_census.rs` fails if a surface touching `window_state_path` is not
  classified as either hydrating or genuinely re-reading.
  **The axis is RETENTION, not persistence** — eleven surfaces persist window state, only
  eight retain a construction read. `theme_editor`, `games` and `programs` read per-call and
  self-heal (the `SettingsModel` shape); that is today's code, not a guarantee, so the census
  pins them too.
  **Two things the Shell's repair proved that the content-site one did not.** (1) **Check the
  absent-branch, not just the read** — `ShellModel::initialize` seeded its default with an
  unconditional `dispatch_write`, and since the sync read *always* misses on the Worker arm,
  merely opening a Shell **overwrote** the persisted `wd`/`history`/`draft`. Data loss, not lost
  session state, and unfixable after the fact. The other seven seed via `seed_state_if_absent`
  (→ `put_if_absent`) and are safe. This half **cannot be gated natively** — one store, one
  authoritative read, so repaired and unrepaired behave identically; its only gate is
  `a_shell_window_returns_to_its_working_directory_on_the_worker_arm`. (2) **Merge, do not
  assign, on any surface holding session-only display state** — `ShellState` carries
  `scrollback` (never persisted) in the same struct as the persisted fields, so
  `*inner.lock() = from_entity(&e)` would adopt the state and blank the screen. Note
  what makes this reachable for windows: a reload restores **no** windows, but **window ids
  restart at 1**, so a re-opened window inherits the last session's
  `workspace/windows/{id}/state`. `make e2e-worker E2E_FEATURES=demo-apps,audit-worker-reads`
  lights the lamp — but it is a **pointer, not a census** (it records subscription *intent*, so
  subscribed-but-unmirrored stays silent). A behavioural gate is what measures the class.
  **Every resolution REPORTS, through `window_hydration::report` — and `Hydration` is five
  outcomes, not four.** Before 2026-08-31 the class shipped with no D13 channel on the arm the
  product runs: `NonePersisted` logged nothing, and the construction-read short-circuit — the
  entire Direct happy path — logged nothing, so a window said how it resolved only on Worker
  and only when it adopted. `AlreadyResolved` was also folded into `Superseded`, so the line
  printed *"superseded"* for a healthy boot and for *"you moved while we were reading"* alike
  (AP40, one layer out from the `bool` this audit already caught). One line now —
  `"window state resolved against the durable tree"`, `surface`/`path`/`outcome` — from all
  three job builders; `every_hydration_outcome_has_its_own_word` asserts five distinct labels
  **and the count**, so a sixth cannot quietly reuse one. **Found by a gate asserting the D13
  channel and failing on Direct while every behavioural assertion passed** — assert the report,
  not only the behaviour.
  Audit: `docs/plans/AUDIT-WORKER-ARM-NAVIGATION-2026-08-30.md` (§11c closes it).
- **`tree put: stored` is NOT durability on the Direct-IDB arm, and that arm is the shipped
  default.** The IDB store is **write-behind** — puts drain on a 250 ms debounce
  (`DEBOUNCE_MS`, `entity-core-rust/core/store/src/idb.rs`) and only identity/destructive ops
  await `IdbCheckpoint::checkpoint()`; an ordinary navigation does not. **Worker/OPFS is
  flush-on-write**, so this asymmetry runs *opposite* to every other split here — do not reach
  for "it's the Worker arm" when a persistence gate flakes. A gate that reloads on the put
  races the flush and fails wearing the costume of whatever bug it guards: measured **1 in 6**
  on the window/Direct location gate, reporting *"a plain reload put the reader back on the
  deployment's home page"* — the hydration defect's own words. **Wait on the store:**
  `durable_state_hash` (`tests/e2e_worker.rs`) reads the `locations` object store and the
  scenario waits for the hash to change from a pre-click baseline; it enumerates
  `indexedDB.databases()` rather than hardcoding a name, because a name that stopped matching
  would wait for nothing and silently reinstate the race — so the Direct arm asserts the probe
  found a hash. 8/8 after. And note the diagnostic trap: **adding a log line made it 15/15**,
  because the extra round-trip covered the debounce. A flake a diagnostic hides is not fixed.
- **A gate can be satisfied by the FALLBACK instead of the repair — check what the boot order
  makes the fallback.** `rekeyed_domain_heals_on_next_boot_window_surface[_on_the_worker_arm]`
  passes with `ContentSiteWindow::hydrate_durable` neutered, on **both** arms: `boot_load`
  spawns the startup window after the session config is final, so a missed construction read
  falls back to the settled config — which in that scenario *is* the asserted page. **When a
  gate's expected value equals its fallback value it measures nothing.** The discriminating
  question is the one `a_returning_reader_is_still_on_the_page_they_left_in_a_window_on_the_worker_arm`
  asks: go somewhere that is not home, come back, still be there (falsified — red under the
  neuter, with the production symptom). Its Direct twin stays green under the same neuter and
  is labelled a **control**, not a second gate.
  **The same coin's other face: a gate can DEPEND on the defect, so the repair reds it and reads
  as a regression.** Measured 2026-08-31 when the window index landed: the phase monolith's
  *"the KB article survived the reload"* check looked only at the **list view**, and the KB comes
  back in list view only because window state was being *lost* — Phase 6 ends in reader view, so
  a KB that correctly restores its own `view_mode`/`current_slug` renders no list rows and the
  check read `count: 0`, wearing the costume of *"OPFS persistence is not end-to-end"*. The
  article was never gone; the assertion had encoded a symptom of state loss as its precondition.
  **Ask what your gate's expected value depends on, not just what it asserts** — the fix was to
  accept either evidence (`via: reader` is *stronger* than a list row) because the subject was
  always *did the bytes survive*, never *which view mode*. Both faces have one root: the gate was
  measuring the fallback.
- **Window state is keyed by an id that is REUSED, so the decoder must check what wrote it —
  AP42.** `next_id` restarts at 1 each session and **a reload is not a close** (only
  `Action::CloseWindow` removes window state), so the entity at `workspace/windows/{id}/state`
  was written by whatever window held that id last session, **of any type**. Decoding by field
  name alone adopts it: measured, the Entity Tree took the Knowledge Base's `expanded_paths`
  (the two mean different things by that name). All **eleven** decoders now open with
  `if entity.entity_type != STATE_TYPE { return <no persisted state> }` — the discriminator
  already existed, `Entity::new("app/state/{type}", …)`, and was being thrown away. **Any new
  window model that persists state adds the same guard and a row to
  `no_window_state_decoder_adopts_another_window_types_entity`** (`window.rs`), which is the
  class-level enforcement point — a matrix, so a new type is covered against all ten others,
  and it asserts `rows.len()` so an omission fails instead of passing quietly.
  **Two window types must not SHARE a state type** — the guard separates readers that disagree
  about their type and is blind to two that agree. Programs persisted `AppViewState` under the
  Apps window's `app/state/games_view`; it now has `app/state/programs_view` and shares only
  the codec (`from_entity_as`/`to_entity_as`). A shared codec is fine; a shared slot is not.
  **Census by `grep -rln window_state_path src/views/`, never by the entity-type literal** —
  the literal grep missed three (`content_site` and `games` had already promoted theirs to a
  constant; `programs` writes through another module's codec).
  Two honest limits: the adoption was **inert in production** (KB paths are relative, tree
  paths are `/{peer}/…`, and `restore_expanded` is additive — coincidences, not guarantees),
  and the guard does **not** stop the last holder of an id overwriting the previous one's
  state.
  **A window id is a SLOT ADDRESS; the durable key is `(type, peer)` — and the indirection that
  joins them is `window_index.rs`.** `{window_id}` was doing two incompatible jobs: the MUST'd
  action wire shape `(window_id, event, value)` wants a small dense reusable integer, and §8's
  persist arm wants something stable over time. Both impls made it a per-session counter, which
  is right for the first job and unsound for the second. The consequence, and the reason AP42's
  type guard is a **guard and not a fix**: *with ordinals and no index, whether you get your state
  back depends on the order you re-open windows in* — the guard turns wrong-adoption into
  no-adoption and never makes the right state findable. One entity at
  `app/{app-id}/workspace/window-index` — type **`app/state/window-index`**, encoded
  `windows: [ { id, content_type, peer_id } ]`, which is guide **§4.2a's ruled cross-impl
  schema** (it was ours; arch landed it near-verbatim). It shipped on `dev` first as the
  app-internal `app/entity-browser/window-index` with `(id, type, peer)` keys, because one impl
  does not name a portable type for something it invented this week; promotion followed §4.1.1's
  own path once the schema existed, and **the Rust struct field names stayed ours**
  (`type_name`) — the ruling is about the encoded map. **No compatibility read, deliberately:**
  an old-shape profile decodes as `Malformed`, which claims and sweeps nothing, and the next
  witness write replaces it — a compatibility read would be a durable record of a transient fact
  (`the_pre_ruling_shape_lands_on_the_safe_arm_rather_than_needing_a_migration`). The wire
  schema is pinned **by literal, not by the module's own `KEY_*` constants**, in
  `the_encoded_map_uses_the_ruled_schema_field_names` — a test spelled in the constants follows
  any rename and can never catch one — and it asserts the row keys as a **set**, because
  `to_ecf` canonicalizes map key order (length, then lexical) and the encoder gets no say.
  It lists the live windows as `(id, type_name, peer_id)` and buys claiming, an
  `next_id` floor, and an exact sweep. `(type, peer)` is not new here — `boot_load` already calls
  that pair *"the stable identifier"* for `BootSurface::Window`, and `find_open` uses it for
  singletons. **Three things it costs to get right:** (1) a boot that re-opens nothing must not
  persist an empty index, or the next boot's sweep deletes every slot — *a window nobody re-opened
  is not a window that was closed* (`WindowManager::retained`); (2) only `Restored` authorizes the
  sweep, and **`no-index` vs `malformed` must not merge** — *"you never had one"* and *"you have
  one and we cannot read it"* differ in exactly the way that decides whether deleting is safe
  (AP40); (3) a malformed row fails the **whole** index rather than yielding a short one, because
  a partial index still authorizes a sweep. The writer is a **witness, not a notification**
  (AP44): recomputed each frame, written on a byte difference, so no spawn/close path has to
  announce itself. Gates:
  `each_window_returns_to_its_own_slot_whatever_order_they_reopen_in_on_the_{worker,direct}_arm`
  — **both are gates, not a gate and a control**, because the defect is arm-independent (unlike
  AP41's); falsified on both arms with the production symptom.
  **Do NOT "fix" this by type-scoping the path** (an earlier handoff recommended it; withdrawn
  2026-08-31) — but **not for the reason this file gave until 2026-09-01, which was wrong.**
  §1's MUST is the `app/{app-id}/workspace/...` **prefix**; the segments beneath it are *not*
  closed (§3.1 blesses two sub-shapes, and workbench-go runs two more without objection). The
  guide's §3 now says so outright. **The withdrawal stands on design grounds:** re-keying does
  not solve two windows of the same type (the ordinal just moves one segment inward); it writes a
  portable type name into a per-app instance path, duplicating what `entity_type` already carries
  — two sources of truth that can disagree, which is what §4.1.1's split exists to prevent; and it
  breaks the one thing §1 *does* pin, since `window_id` would no longer locate a window's state.
  **A design withdrawn for a reason that does not hold is one a later session reopens, correctly,
  and gets wrong** — which is why arch corrected it rather than letting it stand.
  The path itself is still shared —
  `entity-workbench-go/entitysdk/workspace_state.go:397` builds the identical one.
  **§8 is no longer two open arms — it is a checkable disjunction (arch `bd8f463`).** An app
  persisting per-window state MUST be able to say, at startup, which window each persisted entity
  belongs to, and satisfies that by **maintaining an `app/state/window-index` OR sweeping
  `workspace/windows/` before allocating any id**; an app doing neither **MUST NOT persist**
  per-window state. **We satisfy it via the index.** Note what else §8 now says, because it bears
  directly on the still-open resumption question: *the ephemeral arm is a correct and conformant
  choice, not a lesser one.* That decision is open and is the operator's:
  `docs/plans/DESIGN-WINDOW-STATE-LIFECYCLE-AND-SESSION-RESUMPTION.md`. Our per-content-type
  state names are the slot table's long-term shape, so AP42's guard is the convention working,
  not a local invention.
- **A test can ASSERT the non-conformance, and its NAME is what makes it read as a decision —
  AP45.** The two faces above are silent (green by fallback; green by depending on the defect);
  this one speaks. `GUIDE-ENTITY-WORKBENCH-APP` §5.4 rule 3 has been normative since the v0.8.0
  release on **2026-06-21**: a reader MUST log a violation (WARN minimum) on a legacy
  `source_window` / `source_panel` / `content_type` field in a received `app/state/selection`, and
  *"silent tolerance is NON-CONFORMANT."* We dropped all three on a bare `_ => {}` — and
  `from_entity_tolerates_unknown_fields` built an entity carrying `source_window` and asserted we
  **ignore it quietly**. Two months green. *Nobody re-reads a rule a passing test says they
  satisfy.* **The transferable half is the mechanism: one test name spanning two obligations,
  where we met one.** V7 §2.6 open-types really does require unknown fields to be skipped
  *silently*; §5.4 carves three named spellings out of that set and requires the opposite, so one
  test could only assert one and asserted the one already done. **When a rule carves an exception
  out of a rule you satisfy, the exception needs its own test.** Second-order cause: **we were
  conformant on emit and read only the emit half** — §5.4 is five numbered rules, ours was clean,
  the section read as *done*. Enumerate the rules and say which line answers each (1, 3 and 4 now
  each have a test naming its number). **And the trap that decides the fix's shape:
  `content_type` is retired on `app/state/selection` and REQUIRED on `app/state/window-index`**
  (§4.2a says so by hand) — so `legacy_field_violation` is gated on the **entity type**, never the
  field spelling, or our own ruled index schema trips it. Two more choices worth knowing:
  we take rule 3's MUST and decline its MAY-reject (refusing would make one bad emitter a dead
  co-orientation surface, and the reader is not who is wrong); and `Selection::decode` **returns**
  the violations rather than only logging them, because this crate has no `tracing-subscriber` in
  its dev-deps and a pure-predicate test would leave the *wiring* — the half that decays — unmeasured.
  Stated bound: it warns once per decode, and `consume_from_source` decodes every render pass, so
  a legacy entity parked in a slot is loud. That is deliberate; §5.4's stated reason for the rule
  is that silence hides emitters that should be fixed.
- **An idempotent write is not an event — a surface that shows UNPERSISTED state marks its own
  watch dirty (AP43).** The store is content-addressed, so an identical put at the same path
  fires no subscription and is indistinguishable from no write at all. The Shell signalled its
  own rebuild by persisting and waiting for the watch on its own `window_state_path`;
  `record_submit` skips a consecutive duplicate, so **re-running the last command produced a
  byte-identical entity** and the section never rebuilt — scrollback (session-only by design)
  stayed in the model with the `<pre>` frozen on the empty placeholder. Reported for a day as
  *"a Shell after a reload renders nothing"*; warm-vs-cold is not the axis, *did the persisted
  entity change* is. Keep the subscription (it catches an async `exec` completing) — it just
  cannot be the trigger for output the surface produced itself. **Do not generalise:** the
  other five windows that persist without marking dirty are correct, rendering either purely
  persisted state or event-log rows whose paths are always new. And note D24 wants the
  opposite on the Apps surface, where a spurious dirty restarts a running app.
- **If the rule needs the word "every", the structure has to enforce it — AP44.** A guard or
  hook written as *"and also do X here"* is correct the day it lands and decays on the first
  call site added by someone who did not have the whole set in their head. Two instances in one
  session (so: catalog entry, **not** a discipline — it has not bitten twice in different
  circumstances). The hydrate call lived in the content-site *factory*; it now lives in
  `WindowManager::spawn`. The Shell's "did the user move first" guard was a generation counter
  bumped in `persist`, and it read **zero in the very test written to exercise it**, because
  `handle_submit` mutates without persisting — one mutator out of one already missed, before
  any future author existed to forget. **Prefer a witness over a notification:** the Shell now
  snapshots `to_entity().data` (the canonical serialization of exactly the persisted half)
  before the round-trip and compares after — no call site to update, and a new persisted field
  is covered the day it is added. Where no witness exists, put the step on the one
  construction/dispatch path all members already take, and **gate the call site itself** (a
  no-op default means nothing else in the suite can see whether it is wired). The
  counter-example that stops this over-generalising: AP42's type guard genuinely is per-decoder,
  and what saves it is not memory but a matrix asserting `rows.len()`. **If you cannot make it
  structural, make the census fail** — *and then falsify the census.* Third instance, same day:
  `tests/window_hydration_census.rs` asked *"does this directory contain `fn hydrate_durable`"*,
  and when an override was deleted by accident it stayed **green**, because the model carries a
  `#[cfg(test)]` helper of the same name. It scopes to the `impl WindowView` block now and ships
  its own two-way falsifier as a test. **A census you have not falsified reports what you hoped.**
- **A scripted edit to a structured document must be verified by reading back the ROW, not by the
  script reporting success.** A regex-and-replace marking two design-doc rows done printed
  `C9/C10 marked`, passed both of its own asserts, marked one row correctly — and wrote the other
  row's completion note onto **an unrelated row three entries up**, destroying its status cell. The
  board then read the finished item as open and the untouched item as done. Recovered from
  `git show`; verified byte-identical afterwards. **`.replace()` on a match you did not re-locate,
  and offset arithmetic like `old[:-4]`, are the two ingredients.** Prefer an exact-anchor `Edit`,
  or index the lines and assert the line you are about to write starts with the row you mean.
  Same class as a neuter that passes and a gate satisfied by its fallback: *a tool reporting that
  it did something is not evidence that it did the thing you meant.*
- **A documented invocation is a coupling no compiler maintains — run it before you write it
  down (AP37).** Two gates' doc comments instructed `E2E_EXTRA='--ignored'`; the variable did
  not exist, make ignored it silently, and the command printed `0 passed; 2 ignored` — a
  green-looking run of the gates it was meant to execute. `E2E_EXTRA` and `E2E_FEATURES` are
  real now; `WASM_FEATURES` is taken by `make wasm` and means something else.
- **A guard that skips work answers ONE question — check every consequence is downstream of it
  (AP36).** Three defects in this thread were one predicate standing in for two: `put_if_absent`
  for *did the user set this?*, a presence check for *do I hold the CURRENT bytes?*, and
  `home_is_local` for *may we read the domain's document at all?* — the last of which withheld
  origins, supersession revalidation and every log line about the document from every
  local-home profile. **Put the guard on the decision, never on the acquisition.** Boot now
  reads `/entity-deployment.json` unconditionally and gates only the adoption
  (`a_local_home_profile_reads_the_deployment_document_and_keeps_its_home` — whose N-critical
  half reds if the guard is deleted rather than moved).
- **`make e2e-worker T=pulled_demo` is the withdrawn-home gate — AP33.** A deployment's home
  site leaving its publisher's tree while the identity stays put: not a re-key, so nothing
  adopts and nothing heals. Availability survives it (E1, cell #17); **the reports do not** —
  both surfaces name the wrong cause and the cold one gives wrong advice. Run it for any change
  to home-site resolution, the deployment document, or either "site is missing" string. Note
  what it asserts: the surface *speaks*, not that it speaks correctly.
- **`make e2e-worker T=blackholed` is the boot-availability gate** —
  `tools/e2e/blackhole-serve.py` serves an origin that accepts a request and never answers it,
  which `python3 -m http.server` cannot do and which is why this whole failure class was
  previously unreachable from the harness. Run it for any change to the boot path, `sw.js`, or
  `deployment_config`.
- **The recovery console takes EXACTLY ONE action, and the gate is about what it does NOT
  touch.** `?systemrecovery=1` was report-only by design and that was half right: it enumerated
  service workers and Cache Storage and could act on neither, so its printed advice was *"use
  your browser's developer tools"* — impossible on a phone, which is where a stuck visitor is.
  It now has a confirm-gated **"Reset the cached program"** scoped to the two app-code stores.
  **The scope is the whole safety argument:** there is no export path yet, so a reset that took
  IndexedDB or localStorage with it would be strictly worse than the stale worker it clears, and
  unrecoverable. `make e2e-worker T=the_recovery_console_resets_the_program_and_keeps_the_tree`
  asserts the peer databases and localStorage keys are the **same set** afterwards, before it
  asserts anything about the reset working. Run it for any change to that console, to the
  registration block in `index.html`, or to `sw.js`. Three things it cost to get right:
  **(a)** `index.html` registers a worker on *every* load, so the console must skip registration
  when `__ENTITY_RECOVERY__` is set — otherwise the one screen that can remove a bad worker
  reinstalls it on the way in, and only a **re-scan** step in the gate can see that;
  **(b)** the outcome is decided by a **witness** — re-enumerating after — not by what
  `unregister()` returned, and the four outcomes stay apart (AP40): *"there was nothing to
  remove"* must never render as *"fixed"*; **(c)** it does **not** close heal-path row 9 — a
  worker that breaks navigation takes this page down with it, and that is **C17, built and
  drilled 2026-09-02** (see the kill-switch entry below), not this console. The console's own copy
  also told users *"clear site data … only removes the cached program"*, which is **false and
  destroys the tree**; corrected in the same change.
- **A worker that breaks NAVIGATION is a different failure from a worker that caches wrong, and
  only one of them has an in-page fix. C17 — the kill switch — is `assets/sw-selfdestruct.js`,
  shipped with every build and registered by nothing.** Row 9a's recovery console cannot help
  here by construction: it arrives over the same channel the worker has poisoned, and **G8
  asserts that** rather than arguing it (it loads `?systemrecovery=1` through the broken worker
  and requires it to be bricked too). Runbook:
  `docs/RUNBOOK-SERVICE-WORKER-KILL-SWITCH.md`. Drill: `make e2e-worker T=the_kill_switch`.
  **Deleting `sw.js` is not a fix and is the trap** — the spec deliberately does not unregister on
  a failing response, so 404, 410 and a bad MIME type all leave the bad worker installed; the
  server-side kill-switch header was proposed in 2015 and never built. Serving self-removing bytes
  at that URL is the *only* mechanism, which makes *"can you still serve `/sw.js`"* the runbook's
  step 1 and the spec's 24-hour worker-staleness cap the floor when you cannot.
  **Four things it cost that reading the design did not produce.**
  **(1) The drill must not reload after serving the cure.** Recovery has to arrive from
  `clients.navigate()` inside the worker, because unregistering does **not** release a page the
  old worker already controls — a gate that issued its own reload passes with that line deleted,
  and the fix would reach only the people who closed the tab.
  **(2) "The app came back" is not sufficient evidence.** With `unregister()` removed the page
  still recovers, because the self-destruct worker has no `fetch` handler and passes navigations
  through; the registration count is a separate assertion. Same shape as row 9a's witness rule.
  **(3) Read the artifact from the STAGED BUILD, not from `assets/`.** An operator copies it at
  the origin during an incident, so a build that stopped shipping it must red the gate instead of
  quietly testing the source tree.
  **(4) The broken worker in the drill must be checked for actually breaking** — the vacuity
  guard is falsified too, and without it a browser that never installed the bad worker sails
  through every assertion and reports a rehearsed kill switch.
  **No `fetch` handler in the artifact, deliberately:** a worker with no fetch listener intercepts
  nothing, which is the state we are getting back to. **It is lossy and that is accepted** — it
  removes the offline shell — and it touches **no** peer data, the same scope line the console's
  program reset draws, for the same reason (no export path).
- **The health checks live in System Overview's *Problems* card — not a window, and never called
  "Doctor" on screen.** `src/doctor.rs` is the pure verdict logic (checks 1–3 of the resilience
  design §7.2: domain identity, fetch-failure-by-peer, catalog completeness) and
  `src/refresh_ledger.rs` is the session-scoped recording seam a failed refresh lands in. Both are
  native, so `make test` gates the whole product; the window only places the strings.
  **Seven things to know before touching it.**
  **(1) "I could not check" must never render as "healthy"** — `Verdict` has **six** states, three
  of which establish nothing, and none of those three may ever be clear. The gate asserts *that*,
  not a count of clear states: `nothing_that_established_nothing_counts_as_clear` replaced
  `only_agrees_counts_as_clear` when `UserOwned` landed, because a count-based invariant would
  have forced a legitimately-healthy state to lie about itself to stay green.
  **(2) The render filter is `warrants_attention`, NOT `!is_clear()`** — that mistake put three
  non-problems under a heading that says *Problems* on a perfectly healthy profile (AP48). The
  predicate lives on the model so no renderer re-derives it.
  **(3) Anything added to `dom::system_overview::render` must go ABOVE the `!output.tauri` early
  return** or it ships to the desktop app only — which is how the section first shipped, invisible
  in the browser where both incidents actually happened.
  **(4) A remedy may never be destructive** — `no_remedy_is_destructive` asserts it of every
  variant, because there is still no export path (design §5). Remedies report what they *did*,
  never a repair they cannot yet observe — **and "Asked" is not the only thing they can have
  done.** `RemedyOutcome::NobodyListening` exists because the retry told users *"this section
  updates on its own when the retry finishes"* with **no Apps window open**, so nothing could
  finish; the count of listeners is an RAII `RetryHolder` held by the window, i.e. a witness, not
  a registry. Same rule as the recovery console's four outcomes one row over.
  **(5) Check 1 CONSUMES `session_config::decide_home`; it does not re-derive it.** *"Is this
  divergence a fault?"* has exactly one answer in the tree, and when the check computed its own it
  disagreed with the code that acts on it — telling a visitor who had set their own home that
  their profile was pointed at a retired publisher and that reopening would repair it, while boot
  was deliberately repairing nothing. **Enumerating the match arms on one axis is not enumerating
  them**: the arms were exhaustive over every `DocumentRead` and a catch-all over every shape of
  *belief*, which is AP40 hiding inside a function whose doc comment cites AP40.
  **(6) A check that could not see everything says so.** `Snapshot::truncated` had two readers and
  check 2 ignored it, returning `Agrees` off a capped list — the exact thing that field's own doc
  warns against. And note *why* no test caught it: the fixture helper hardcoded `truncated: false`,
  so no check-2 test could construct the other half of the field's domain (AP39).
  **(7) The browser gate is `make e2e-worker T=the_problems_card`, and it runs on the BLACK-HOLE
  RIG for the same reason the boot-surface gate does.** Both defects this surface has actually had
  were invisible to every test of the model (AP48), so the gate asserts what a person sees:
  the card is **there in a plain browser** (N: move it below the `!output.tauri` return → red),
  it carries **zero findings on a healthy profile** (N: filter on `!is_clear()` → red, with the
  three non-problems printed), *Check again* **has a visible state** (N: ignore `checking` → red),
  and a check that **could not run is put in front of the user** (N: drop `Undetermined` from
  `warrants_attention` → red, showing *"No problems found"* while a check could not run). It reads
  the card by **text**, anchored on the `h3` — there is no class to select and adding one would
  measure the hook. The stall is load-bearing twice: `DocumentRead::Unheard` needs an origin that
  *accepts and never answers*, and the ~3 s deadline is what makes the *Checking…* window
  observable rather than a race. **Still uncovered:** the remedy button reaching an open Apps
  window — that needs check 3 to fire, i.e. a publisher that withholds a set, which is a different
  rig; `RemedyOutcome` is gated natively. And **`doctor.rs` was English-only by decision and is
  translated into all 30 locales as of 2026-09-03** — `tools/i18n-lint-baseline.txt` is now
  **empty**, the floor, and `make lint` reads `raw=0`. Its count had gone **UP** (59 → 66) on
  2026-09-02, deliberately and on the operator's call, because the audit's fixes needed new
  outcomes to have their own words: the one time this ratchet has been spent upward, and it was
  spent against a stated deadline (*translate before release*) which is what made it a loan rather
  than a concession. **The pattern is the transferable half — a debt with an instrument behind it
  gets paid; one that lives in a sentence does not.** The mechanical cost was `copy`'s items going
  from `const &str` to functions, since a `const` cannot consult the active locale; nothing about
  the design moved. **Keep the reasoning in `doctor.rs` and the wording in the catalog** — a
  translator needs to know that a verdict must not read as reassuring and that two outcomes
  rendered alike lose the distinction they exist to carry, and a JSON catalog has nowhere to say
  it. Note the standing limit this does not touch: `i18n-untranslated` covers **13 of 30** locales,
  because in the 17 Latin-script ones a cognate is indistinguishable from a skipped string.
- **A value with no PROVENANCE cannot be refreshed from its source, and a heuristic on the value
  is not provenance — D25** (ratified 2026-09-02; AP49 stays as the incident record).
  `MODEL-STAKEHOLDERS-AND-OWNERSHIP` §2.2 states it outright: *a value
  a deployment seeded and a value the end user deliberately chose are byte-identical in the
  entity.* The warm-boot reconcile refreshes exactly one such field, `home_site`, and it guarded
  on `home_is_local` — which reads *"the user set this"* as *"the user set this to something
  **local**"*. So a user who picked a **cached foreign** site (the boot-target picker offers them,
  labelled `"{site} (cached)"`) had it overwritten on the next boot **and** got a durable
  supersession record naming their own choice as *retired*, which then rewrote every stored
  reference to it. **No re-key required** — the branch is `new_peer != stale_peer`, true the
  moment the user's choice differs from the domain's declaration. The home reverting is E1; the
  supersession is not (revalidation keeps it, because the domain does not contradict it).
  **The mechanism was already built, twice, for the same question one field over** — that is what
  makes this an anti-pattern and not a bug: site origins carry `source: deployment | user` with
  `Adoption::KeptUserOverride`, and the registry pin has `pinned_registry() -> (RegistryPin,
  PinSource)`. Four answers to one question in one codebase is the failure the stakeholder model
  was written to stop, and it names Partition B as *"the gap to close for refresh semantics"*.
  **Do NOT close it by building Partition B** — the model says so itself (*"not a prerequisite…
  treating it as one over-scopes that work"*). One field, one mark.
  **Two enforcement points, and the second is the transferable one.** `session_config::decide_home`
  is now the single, **pure** expression of the precedence (five outcomes, not a bool — *kept it,
  the user chose it* / *kept it, it is local* / *nothing to do* are three different facts), so
  every combination of provenance × declaration is gated by `make test` on both arms instead of
  only through Selenium. And `every_deployment_declared_field_says_who_owns_it`
  (`deployment_config.rs`) is a **census**: one row per field the document may declare, each
  classified as deployer-only / user-may-override-by-*named mechanism* / adopted-only-on-first-
  contact, with the count asserted — so adding a field to `/entity-deployment.json` fails the
  build until someone answers *whose value wins on the next boot*. A row claiming a mechanism must
  name it; a claim with nothing behind it is the defect the census exists to catch.
  **AND IT WAS VACUOUS UNTIL 2026-09-07 — a census's own claim about itself is the last thing
  anyone checks.** It asserted `rows.len() == 9` against the hand-written vec **directly above
  it**, with nothing connecting either to `DeploymentConfig`, while its comment read *"a field
  added to `DeploymentConfig` without a row here fails"*. It would not have. Exactly the shape of
  the Doctor roster's `all.len() == 3` closed five days earlier, in the census D25 was ratified
  on — *a rule with an enforcement point is only as good as the enforcement point being real*, and
  the two most load-bearing censuses in this repo had the same hole at the same time. Found while
  adding a tenth field, by the author of the field, which is the one moment it is cheap.
  **Closed by making the compiler two thirds of it**, and the loop now has no manual link:
  an **exhaustive destructure** of `DeploymentConfig` (a new field is `error[E0027]`, naming it),
  **`#[deny(unused_variables)]`** on the test (binding it without listing it is a second compile
  error), then `rows.len() == declared.len()` where `declared` is built **from the bindings**.
  Falsified — a dummy field reds with `error[E0027]: pattern does not mention field`.
  **The transferable move: when a census counts a literal, ask what would make the count wrong,
  and make *that* a compile error.** `assert_eq!(rows.len(), N)` where `N` is typed by hand is
  measuring the author's memory.
  Gates: `a_home_the_user_chose_is_not_overwritten_by_the_deployments_declaration` (browser,
  through the real Settings picker — falsified both ways, on the writer half and the reader half
  independently) and `a_user_chosen_home_is_kept_and_a_deployment_seeded_one_is_adopted` (native,
  one bit apart, so incident A's repair cannot be lost to this fix).
  **Unmarked reads as `Deployment`**, for B1's stated reason, and the cost is real and stated: a
  user who chose a remote home under an older build is not protected until they choose again.
  **Ratified as D25 on the ladder's early-promotion terms**, recorded rather than dropped: the
  second instance was found and fixed by the session that wrote the rule, which is when a rule is
  most likely over-generalised — so it is reviewed against a third instance found by someone else
  and removed if unearned within a release cycle. What the census does **not** see is a value
  adopted from a registry or a peer; it only covers fields the deployment document declares.
  Audit: `docs/plans/AUDIT-HEAL-PATH-AND-THE-OWNERSHIP-GAP-2026-09-01.md`.
- **A ROUTING fact the client never re-reads is a correct publish that never arrives — and the
  ownership census could not see it, because it had one column (AP50).** D25's census
  (`every_deployment_declared_field_says_who_owns_it`) asks *whose value wins on the next boot*.
  It did not ask *does the deployer's value ever arrive*, and **the two fields that answered the
  first question best were the two whose refresh was broken**: `name_registry_pin` was adopted
  only inside `HomeDecision::AdoptDeclared` — so a deployment that added or moved its registry pin
  reached a returning profile only if the **home peer** changed in the same publish, an unrelated
  trigger — and `name_resolver_max_ttl_ms` was refreshed on **no** warm boot at all, only by
  `apply_to` on a cold one. That is the §6a ceiling protecting the consumer *against* the
  registry, so a deployer tightening it reached nobody who had already visited. **The deployed
  document already names two peers**: `entitychurchfoundation.org` declares a pin whose peer id
  differs from its home peer.
  **The direction of the miss is why it survived three audits of this area.** Everything else in
  this arc is *the client acted on something stale*; this is *the client never learned something
  true* — nothing renders wrong, no request fails, and the profile that has never booted is fine
  while the returning one is not. It is the `origins` CDN-move bug once per field left out.
  **It was deferred in writing, not forgotten, and that is the transferable half.**
  `DESIGN-RESILIENCE…` §1.1b's *"What it does NOT cover"* said so outright. The deferral lived
  only there, while the instrument built two weeks later for exactly this class did not encode it
  — and **a census that is green reads as coverage**. Same shape as `make lint` not compiling the
  e2e suite while this file recorded the `make test` half of that hole: *recording a gap is what
  makes it look handled.*
  **The fix is a column, not a patch.** `session_config::decide_routing_refresh` is the one pure
  expression (three outcomes — `Adopt | Unchanged | NotDeclared`, and the last two must not merge:
  *the document agrees* and *the document said nothing* are different facts, and D23's deadline
  makes the second more common). It is wired in `boot_phase2` gated on **a document was read**,
  the same reachability as the origins loop, never on what the home did — and the pin's adoption
  was **removed** from the `AdoptDeclared` arm rather than duplicated, because two writers of one
  value is the drift shape C15 exists to stop. The census now carries
  `Refreshed::{EveryWarmBoot, FirstContactOnly}` with the routing set asserted, so a new field must
  answer both questions. **The trust boundary is unchanged and bounds this:** routing is adopted,
  posture is not — `surface`, `window_type`, `site_mode`, `fast_paint` and
  `peer_creation_enabled` stay first-contact-only, each now with its reason on the row.
  Gates: the decision natively on both arms, plus
  `make e2e-worker T=a_deployment_that_moves_its_registry_pin` — falsified, reporting the FIRST
  pin after the second was published. Its document declares **only the pin and no `home_site`**,
  deliberately: with a home declared, `AdoptDeclared` fires and the gate passes under the defect,
  which is a gate satisfied by its fallback. **Stated bound:** the browser gate covers the pin; the
  TTL shares the block and the function but has no surface that displays it, so its wiring rides
  the pin's falsification of that block rather than an assertion of its own.
  Design: `DESIGN-RESILIENCE-RECONCILIATION-AND-ENTITY-DOCTOR.md` §1.1e.
- **THE PRODUCT DIAGNOSED IT CORRECTLY AND TOLD A STATUS CARD INSTEAD OF THE RESOLVER — AP54,
  found in PRODUCTION on 2026-09-05, by devops, on the first real re-keyed deployment.** The heal
  path shipped, was audited, and was signed off. It healed the home and the content-site surface.
  **The Apps surface never healed at all**, and on the same boot, in the same session, Entity
  Doctor's check 2 was reporting `fetch-failure-by-peer: diverges` and **naming the dead peer** —
  the exact fact `app_source` needed, already loaded in memory in phase 1, before the Apps window
  spawned. It was not a race. *Diagnosis reaching a card is not repair.* **When a check can name a
  fault, ask which code path would have to consume that same fact to FIX it, and whether anything
  does.** A health surface that is right while the resolver is wrong is a signal you have built the
  knowledge and not wired it.
  **Three defects, one root, and all three verified in source before being believed:**
  **(1)** `peer_supersession::resolve` had exactly **two** call sites, both in
  `views/content_site/model.rs`. **(2)** `app_source` preferred the origin *"whose catalog we
  already hold — a stable choice across frames"*; on a re-keyed profile the peer you hold a catalog
  for is the **retired** one, and it is **self-reinforcing**: the live peer can only earn a catalog
  by being fetched, and the fetch is what the preference refuses to make. **A preference for state
  we already hold is a preference for STALE**, and after an identity change it never recovers on
  its own. **(3)** `list_origins` did no supersession filtering, so the dead registration survived
  every boot — the adoption path only ever *adds*.
  **The commit that introduced it said the right thing and did not do it, which is worse than
  silence.** `ac515ff`'s message reads *"a replaced publisher is ONE fact about a peer, not N facts
  about surfaces."* Its code is a fact about one surface — the one the author had open. **A commit
  message stating the general principle is not the principle being implemented**, and it makes the
  scope look considered to every later reader, including the audit that signed this off. Grep the
  call sites; do not grade the rationale.
  **Enforcement, structural rather than conventional:** the resolve moved into
  `origins::list_origins`, which **all seven** call sites already use, and
  `list_origins_raw` is **private** — there is no way to obtain an unresolved listing from outside
  that module, so a surface added tomorrow inherits it without knowing it exists. The decision is
  `apply_supersession`, **pure** and taking the resolver as an argument (same reason `resolve_in`
  was split from `resolve`), so every combination is gated by `make test` on both arms.
  **Read-time resolve, never a stored sweep** — writing the resolution down would be a durable
  record of a fact the live document owns (AP30), and revalidation exists to drop a record the
  domain contradicts.
  **The middle case is the one a tidy fix gets wrong:** superseded **and the successor is not
  separately registered** → **carry the origin forward**, do not drop. A re-key is normally the
  same publisher at the same origin, so dropping the only row naming it turns a recoverable re-key
  into an unreachable publisher — availability lost to hygiene. Drop only when the successor has
  its own registration, which `adopt_deployment_origin` wrote from the live document and which
  therefore outranks anything we inferred.
  **The gate's PRECONDITION is the gate** (devops asked for exactly this, and they were right):
  `a_rekeyed_domain_stops_serving_apps_from_the_retired_publisher` **warms the retired peer's
  catalog first**. Without that, `app_source`'s first loop finds nothing, falls through to *"the
  first foreign origin"*, and the answer depends on which peer id sorts first — **an unwarmed
  fixture passes with the defect fully present.** The retired peer is also chosen to sort *first*,
  so sort order cannot supply an accidental pass. Two traps building it: `catalog_path` is
  peer-qualified, so a **placeholder peer-id string makes the warming `put` a silent no-op** and
  the precondition unreachable — use real generated ids; and warm through the `WriterHandle`, which
  is the path `foreign_cache::ensure_current` actually uses. Falsified: neuter `list_origins` back
  to raw and both gates red with the production symptom.
  **The wrong sentence, fixed and bounded:** the surface said *"the publisher withheld it"* about a
  publisher that was **retired**, whose replacement was serving those catalogs at 200. *"This
  publisher does not carry it"* is a claim about a live publisher's choice; asking a dead one is
  our mistake, not their decision (AP40). That arm now checks `is_retired` — belt-and-braces, since
  the chokepoint stops us asking at all, but still reachable when phase 1's supersession read fails
  and leaves the dead row *unresolvable* rather than merely unresolved.
  **Stated and deliberate: the retired peer's cached catalogs are NOT swept.** They stop being
  consulted and are inert. Deleting them is the destructive direction with no export path, and D24
  is explicit that a cache which drops what it cannot re-verify turns an outage into a missing app.
- **SUCCESSION IS DECLARED, NEVER INFERRED — `superseded` in the deployment document, shipped
  2026-09-07 (§1.1f items 1+2).** The home peer's re-key is the one case a client may infer,
  because `home_site.peer_id` is a **single slot** and a new value in it *means* replacement.
  `origins` is a **map**, where a key vanishing as another appears is ambiguous between a re-key
  and one tenant leaving as another joining — and guessing writes a supersession against a peer
  that is alive, which is F2's brick with a wider trigger. So the deployer says it:
  `publish --deployment-config --supersede=OLD=NEW` (repeatable), read by
  `DeploymentConfig::superseded`, adopted by `peer_supersession::adopt_declared` in `boot_phase2`
  **before** revalidation, on the same ordering invariant as the inferred home re-key.
  **CURRENCY IS WHAT THE DOCUMENT AFFIRMS, NEVER WHAT IT ROUTES TO** —
  `current = {home} ∪ values(declared) − keys(declared)`, and an `origins` key is deliberately
  **not** in it. **This cost five red e2e gates and it is the entry to read before touching
  `stale_against_declared`.** The obvious definition is *every peer in `origins`, minus the ones
  declared retired*, and it is wrong in the most common case there is: after a re-key the document
  names the NEW peer as home and **keeps the retired one's `origins` entry on purpose** so old URLs
  resolve while visitors roll over (`HomeClaim::Takes` preserves sibling origins). Read that as an
  affirmation and you contradict the record the *inferred* home adoption just wrote, and delete it
  on the same boot — every failing gate was a re-key gate, on a change whose purpose was to make
  re-key recovery work for *more* peers.
  **The transferable half is the shape, not the subsystem: the wrong version already knew *hosted
  is not current* and expressed it as a subtraction.** It just only covered the case that had a
  *declaration* to subtract, while the inferred re-key makes the identical shape with nothing to
  subtract. **A subtraction that exists to repair a set is a sign the set is the wrong set** — ask
  what set would not need it. Replacing it removed the special case instead of adding a second one.
  Pinned by `the_retired_peer_still_being_routable_does_not_contradict_the_record` (written before
  the fix, seen red), with `a_record_against_the_affirmed_home_is_still_dropped` and
  `withdrawing_a_declaration_drops_the_record_it_created` as the opposite faces — the second is
  F2's escape, and it works *because* of the affirmation definition. `stale_against` is now a
  one-peer call into it, so a single-publisher document behaves exactly as it did.
  **The emitter is ADDITIVE on every arm, including the home publisher's**, which rebuilds the
  domain's own fields from scratch. A succession dropped by a later publish is **silent** —
  nothing 404s, nothing renders wrong, it just never reaches anyone — the `origins` clobber one
  field along and much harder to notice (`a_declared_succession_survives_a_later_publish_by_another_peer`,
  falsified). A document declaring nothing carries **no** `superseded` key, so *"declared nothing"*
  and *"declared an empty set"* stay apart.
  **One stated bound: authority is by who holds the out-dir, not by signature**, so nothing stops
  a secondary publish declaring succession for a peer it does not own. (The declared path is
  gated in a browser as of 2026-09-07 — next entry.)
- **`make e2e-worker T=a_declared_succession` is the DECLARED path's browser gate, and the way it
  was built is the transferable half.** Its document declares a succession and **no home change**,
  which is what makes it a falsifier rather than a re-run of the seven inferred gates: the only
  thing that can put a record in the map is `adopt_declared`. **No multi-peer publish was needed**
  — the peers are synthetic, because a supersession key is a *deep* path segment, not the
  peer-qualified first segment that has to be a real id.
  **A NEUTER THAT PASSES HAS A THIRD CAUSE: the gate does not distinguish what you thought it
  did.** Deleting `revalidate` left the withdrawal step **green** — not because the neuter missed
  and not because the gate was unsound, but because `adopt_declared` re-writes the record on
  **every boot that declares it**, so *"gone after a withdrawal"* is equally explained by *"never
  durable, and simply not re-adopted."* The fix is a **step, not an assertion**: withdraw the whole
  document and boot, so nothing adopts and nothing revalidates and a record still on screen is a
  durable one. It also gates `revalidate`'s stated *no document means no change* precondition,
  which nothing had ever exercised in a browser. And note the obvious neuter lands somewhere
  unexpected: an **empty** declared set to `revalidate` reds the *adoption* step, because with
  nothing affirmed but the home the record is dropped on the boot that adopted it.
  **`put_and_wait` returning is not durability, and a `goto` loses the write.** The Direct-IDB arm
  is write-behind (250 ms debounce), so the first cut lost the record *and* the session config to
  the navigation after phase 2; boot 3 came up on the build default and the console correctly said
  *"could not determine"* for a profile that had never written anything down. **Wait on the store,
  on the APP page** — navigating away abandons the pending drain, so waiting from the recovery
  console waits for something that can no longer happen. `durable_hash_for` is the probe.
  **Order the assertions by the diagnosis they give**, not by the outcome: the subject assertion
  reads the routing mirror on the app page *before* the durability waits, because with
  `adopt_declared` unwired the waits would time out first and blame the probe.
- **THE §6.5 ESTABLISHER WAS A BOOT-ONLY DECISION, SO EVERY FRESH PROFILE WAS FINDABLE AND
  UNREACHABLE — fixed 2026-09-07 (`src/late_establish.rs`).** Reported as *"they detect each other
  but chat doesn't work"*, in a private window and on the desktop, and read as a regression. Nothing
  had regressed: `git log -S meet_no_establisher` lands at `043e56d`, 2026-08-24, untouched.
  **The seam is a CONSTRUCTOR argument** — `new_direct_idb_with_establish` takes it because it must
  be captured before the peer's `PeerShared` clones, and there is no `&mut Peer` on this arm — and
  it was built at boot from `resolve_provisioning`: URL → the **localStorage selection mirror** →
  the build knob. **A fresh profile has none of the three**, so no seam was installed, and a
  connector chosen afterwards could not reach the running peer. A warm profile carried the selection
  in localStorage and worked, which is exactly why it read as *"it used to work"*.
  **Discovery and reachability are separate mechanisms, and only one was broken.** `meet` is an
  ordinary websocket call to the rendezvous node and needs no establisher, so peers found each
  other and the roster lit up while every connect-back was structurally impossible. *A roster
  entry is not a transport.*
  **The fix installs the seam ALWAYS, empty.** §10.3's own contract is what makes that free —
  *"returning `None`, including no establisher registered at all, makes the ladder byte-identical
  to the pre-seam behavior"*, and `Err` is *"a reason, never a branch"* — so an unarmed slot answers
  `NotAttempted` and an unprovisioned boot is unchanged. The seam is consulted **per dispatch**, so
  filling it later is picked up by the next attempt. **Stated cost:** an always-present slot accrues
  failed consultations against the kernel's sequential backoff — bounded by its own constants, 10
  free consultations and a **10 s** cap, so the worst case is one attempt arriving late.
  **`peer_has_webrtc` follows the ARM, not the slot existing (AP40).** The constructor no longer
  infers it: with a late slot always present, *a seam exists* and *a node is configured* became
  different facts, and conflating them would make every meet warning permanently silent — the
  opposite of the defect being fixed. `Peers::set_webrtc_peer` is how a caller declares it.
  **Report from the ARM, never from the decision.** Logging at the call site off the returned
  outcome looked equivalent and is not — a neuter that returned `Armed` without arming printed
  *"armed"* over a session that armed nothing. The line moved next to `slot.arm()`.
  **THE LINUX DESKTOP CANNOT DO WEBRTC AT ALL, and that is measured, not inferred.** WebKitGTK
  ships without the bindings compiled in — `RTCPeerConnection` **undefined** on Debian 2.50.6 and
  Fedora 43 2.50.5, with `MediaStream` present, which is what makes it diagnosable — so a Tauri
  window is a rendezvous **node** and a websocket peer, never a WebRTC peer. `MeetReach::NoWebRtcApi`
  outranks every configuration answer, because telling that user to add a connector or reload sends
  them to fix something that is not broken. `src-tauri`'s own note says why this cost three sessions:
  *"nothing fails loudly, and the half that keeps working is the half you look at."*
  **Gates:** `decide_late_arm` and `MeetReach` are pure and native (four and five outcomes, counts
  asserted), `late_establish`'s own tests cover the slot, and **`make e2e-webrtc-meet-noreload` is
  the behavioural one** — the meet-then-chat journey with step 3's reload **removed**, so two
  browsers boot bare, add a connector mid-session, and chat. Direct arm only, and the spike
  **refuses** on Worker rather than passing (there `InitParams` really is Init-only). Falsified
  twice; note the discriminating assertions are **message delivery and the unreachable note**, not
  the arm log — a detached-but-armed slot still logs, which is why that check is labelled a step
  indicator.
- **A RELOAD IN A TEST SETUP IS A WARM-UP STEP — ASK WHAT IT WARMS (2026-09-07, AP55).** Every
  WebRTC gate here boots warm and none of them said so: `spike_chat_over_webrtc` is handed
  `?webrtc_node=` (the *top* of the precedence chain, installed at boot), and
  `spike_meet_then_chat` + `spike_file_over_webrtc` both *"add the connector, then reload"* — and
  `goto()` is **precisely the step that turns a fresh profile into a warm one**, because
  `resolve_provisioning_quietly`'s middle source is the localStorage selection mirror, which is
  readable only at boot. So the population contained no cold profile at all, which is why the
  boot-only establisher shipped, was audited, and stayed green for two weeks (AP34/AP35's shape:
  green because the failing configuration was not in the population, not green by inheritance).
  The reload was even **documented as load-bearing** — true, and it was load-bearing *for the
  defect*. **`make e2e-webrtc-file-noreload` closes the half that mattered**: the meet gate proves
  a late-armed seam carries one chat entity; this proves it carries a **multi-chunk closure walk
  over `system/content`**, which is what the report was actually doing. Falsified — the neuter
  (install the seam only when boot already resolved a node) reds with the production symptom
  exactly: ✅ met each other, ✅ a browser can serve a file, ❌ *every* delivery assertion, with
  the in-session arm still logging `True/True`. **Run both `-noreload` gates for any change to
  `late_establish`, provisioning precedence, or the connector registry.**
- **A WORKAROUND IN A RUNBOOK IS A BUG REPORT NOBODY FILED (2026-09-07).**
  `RUNBOOK-TWO-MACHINES` §5.1 carried *"Reload Tori once after step 1… provisioning is read at
  boot, so until you reload, the UI can find peers and they cannot connect back to it."* That is
  the establisher defect, diagnosed **correctly and in writing**, ~2 weeks before it was reported
  as *"chat regressed"* and read as a regression. Someone understood it exactly and shipped a
  step instead of a fix. **When you write "reload once" into a procedure, ask what is being
  reloaded and whether the running process could just be told** — and grep the runbooks when you
  fix something, because the workaround outlives the bug and keeps teaching it. That line was
  also stale a *second* way nobody had noticed: on the Linux desktop `RTCPeerConnection` is
  undefined, so the reload promised a repair that surface could never perform.
- **A STALE BELIEF SILENCES THE MACHINERY THAT WOULD CORRECT IT — the sleep/wake bug, fixed
  2026-09-08 (`src/wake_probe.rs`).** Two machines chatting over §6.5 WebRTC; both sleep; on wake
  each shows the other offline and nothing recovers until a reload, which recovers it in ~1 s with
  no need to re-`meet`. **Nothing downstream was broken.** The §4.1 continuations, the §2.2
  backoff, the §10.3 ladder and `reach_keeper`'s presence all work — they are never *triggered*,
  because the liveness transition that starts them is not written for **~130 s**
  (`3 × (30 s + 10 s) + 10 s` grace, `KeepaliveConfig::default`). The reload was never repairing
  corrupted state; it was **skipping a slow detection path**.
  **The deadlock is the transferable half, and each half is individually correct.**
  `ReachKeeper::due` skips any peer the kernel calls `Connected` — right, since presence is for
  peers we cannot reach — but after a suspend that is *every* peer, so the app's own recovery is
  switched off for exactly the window it is needed. Meanwhile dispatch keeps using the dead pooled
  route, so the ladder is never consulted either. **`is_connected()` was answering two questions:
  *do we need presence?* and *is this belief current?*** (AP36 — a guard that skips work answers
  ONE question; check every consequence is downstream of it.)
  **The keepalive cannot short-circuit it, and an app dispatch can.** A missed ping only
  increments a counter and it takes `max_missed` of them; a transport error on a connection we
  *believed active* runs the kernel's §A1 seam (`demote_peer_on_transport_error`, `core/peer`'s
  §10 step-1 site), which evicts the binding and writes `suspect` on the **first** failure. So the
  whole fix is **send something on wake** — no kernel change, no app-side eviction, and we write
  no liveness at all: the kernel observes the send failing and owns the demotion, which is the §A1
  seam discipline rather than the fourth parallel liveness store AP12/D8 refuses.
  **Probe, do not evict** — tearing down on wake costs a full re-establish on every wake where the
  link was fine; the value was never in *which* action but in not waiting up to 30 s for the next
  tick, and both options capture that. **~130 s → one round trip.**
  **The wake signal is the FRAME LOOP, not the watchdog** — rAF does not advance while suspended
  or backgrounded, so a large wall-clock gap between frames *is* the resume, with no worker, no
  `visibilitychange` (a suspend does not always fire one) and no dependence on a watchdog the user
  can turn off with `?watchdog=0`. The watchdog reaches the same conclusion by its own route and
  **reports** it; it deliberately does not also act, because two detectors driving one repair is
  the parallel mechanism. The threshold is **derived** from `KeepaliveConfig::interval_ms` (§5.4's
  own "recently active" window) rather than typed in, so a §12.4 retune moves both together — and
  it doubles as the debounce, which bounds the cost exactly: ≤1 wake probe per keepalive interval
  can never more than double traffic already flowing.
  **THE MECHANISM WAS ALREADY IN A TEST, AS SCAFFOLDING.** The reconnect proof's `cross_peer_probe`
  carries the comment *"Provoke the drop detection: one dispatch over the dead connection… without
  this we would be waiting ~60s on keepalive."* Understood exactly, written down as a workaround
  **inside a test**, and never made a behaviour the product performs — *"a workaround in a runbook
  is a bug report nobody filed"* one layer in. **Grep your test scaffolding for the fix**: a step a
  test must perform to make the product behave is usually a step the product owes the user.
  Gates: `decide`/`wake_gap_threshold_ms` and `FreezeVerdict` are pure and native, and
  `a_connection_that_died_is_only_noticed_when_something_dispatches_at_it` (`peers.rs`, memory
  transport) is the behavioural one — **falsified**, reporting *"it is still `connected`, so the
  ~130 s wait is back"*. **Its step 3 IS the gate:** asserting only that the probe demotes would
  pass equally if the kernel had already noticed, so pinning that the belief is *still* `Connected`
  first is what makes the probe **necessary** rather than merely present.
  **Stated bounds, because this class has never been gated in a browser.** No rig here can suspend
  a machine, and `make e2e-webrtc-idle` — written for exactly Amendment 14's idle-death case —
  **exits 2 = INCONCLUSIVE by its own Makefile note**, because `ChatDelivery`'s 5 Hz poll keeps the
  link busy so nothing ever goes idle. So the *detection* claim is gated and the *suspend produces
  the frame gap* and *NAT mapping expiry* claims are not. Retiring that poll onto `reach_keeper` is
  what would make the idle gate mean something; it was **not** done here — the A3 finding makes it
  a separate investigation (the poll is load-bearing for establishment retry and for symmetric
  delivery), and blocking a recovery fix on it would be the larger mistake. Do not let a green
  suite imply coverage this class has never had.
  **Worker-arm bound:** the believed-connected set is read with the sync `read_peer_liveness`,
  which on the Worker arm answers from a mirror seeded only for watched prefixes — so a profile on
  `?worker=1` with no connection-showing window open probes only its `reach_keeper` intents. The
  shipped arm is Direct/IDB, where the read is complete.
- **A GATE WRITTEN TO PROVE A FIX WORKS IS THE ONE THAT FINDS IT DOES NOT — 2026-09-08,
  `make e2e-webrtc-vanish`.** It was written as the positive half of the WebRTC EOF change
  (item C), which had shipped an hour earlier with a commit message claiming a dead channel now
  "fails now rather than on the next 30s request deadline". **It does not.** The gate's own
  diagnostics locate the gap precisely — `transport is over: 1` (our sentinel IS posted) with
  `terminating reader task: 0` (the reader loop does not end) — so the wiring is a *prerequisite*
  for the claim, not the claim. **Write the gate before the commit message, or the message is a
  hypothesis wearing a result's clothes.**
  **The worse finding is the one nobody was looking for: detection is BIMODAL, and a vanished
  peer's counterpart just keeps rendering Connected.** Over 8 fresh-grid runs, ~half detect at
  30.6 s and the rest not within 75 s. **Diagnosed the same day to a `core/peer` bug — not a
  spec issue and not the SDK** (`docs/status/HANDOFF-2026-09-08-A-VANISHED-PEER-IS-NEVER-NOTICED.md`):
  `spawn_reader_loop` terminating has **no consequence**, so the pooled binding stays live, the
  status entity still says `connected`, `ReachKeeper::due` keeps skipping the peer, and every
  later dispatch burns the full 30 s `DEFAULT_REQUEST_TIMEOUT`. **Evict on reader-loop exit; do
  NOT write liveness there** — §A1 forbids demoting inside a transport primitive, and eviction
  alone feeds machinery that already exists (§5.4a's `escalate_unbound_suspect`, the §10.3
  ladder, `reach_keeper`). *The spec had specified the recovery; nothing triggered it.*
  **A ZERO FROM AN UNVALIDATED NEEDLE IS NOT EVIDENCE, and this cost a wrong published
  conclusion.** The first pass read `terminating reader task: 0` and wrote up *"the reader loop
  does not end"* — into a commit message. Adding a **control needle** for a kernel line that MUST
  be present (`internal dispatch: remote completed`, 24 hits) proved capture was fine, and a
  temporary probe in `connection_from_port_typed` then showed the sentinel arriving and the
  reader ending after all. **Put a must-be-present control in any log-grep diagnostic panel**,
  and note the twin trap: **the instrumented run changed the mode**, so the probe was removed and
  the measurement repeated before anything was concluded.
  **Landed RED on purpose, and the threshold was not tuned to make it green.** 40 s and 75 s were
  both tried; 75 s fails too, and a budget above 30 would only be measuring the request deadline —
  a gate satisfied by its fallback. **The gate is deterministic; the PRODUCT is what is
  nondeterministic, and that distinction is what makes a red gate legitimate rather than flaky.**
  **CLOSED 2026-09-08 at `6/6 PASS, 0.5 s`, by two `core/peer` fixes and no threshold change** —
  the entry below is what it took, and the fix shape recommended above was wrong.
  **Two rig lessons it cost.** Its first cut failed on its own premise — `(no-row-for-peer)` —
  because Peer Connections' *Known devices* table is written by `reach_keeper`'s remember branch
  and therefore needs a reach **intent**, which binding a chat by peer id never registers. The
  Chat window's `[data-field='chat-reachability']` row needs no registry and is where the person
  who reported this was looking. And the diagnostic needle `"reader task terminated"` matched the
  *awaiter's* message, not the reader's own `"terminating reader task"` — **a zero from a needle
  you have not verified against the source string is not evidence.**
- **A TRANSPORT PRIMITIVE THAT EVICTS ITS OWN BINDING DISARMS THE DEMOTION THAT WAS ABOUT TO
  FIRE — the vanished-peer fix, 2026-09-08, and the handoff's recommended fix was this mistake.**
  That handoff said *"evict on reader-loop exit; do NOT write liveness there"*, reasoning that
  §5.4a's `escalate_unbound_suspect` would take it from a missing binding. **It would not, and
  the eviction is what stops it.** `demote_peer_on_transport_error` fires only while the failed
  endpoint is **still the bound one** (the §A1 no-clobber guard), so evicting from the transport
  makes the next dispatch's demotion a no-op; and `escalate_unbound_suspect` escalates only from
  `suspect`, which nothing then wrote. Measured rather than reasoned: with the binding removed at
  reader-death the seam leaves `system/peer/status` reading **`connected`**, with no mechanism
  left to move it — strictly worse than the bug. **So the primitive REPORTS and the seam ACTS:
  the reader raises a flag, and a dispatch issued after it fails immediately instead of at the
  30 s deadline.** *Do not fix a stale binding by removing it from the layer that cannot demote
  it.*
  **The second half is why it read as a 50/50 flake: a §6.5 link is ONE connection with TWO
  handshake roles, so ONE defect has TWO code paths and only one was fixed.** The same vanished
  counterpart is a dead `RemoteConnection` on one browser (`spawn_reader_loop`) and a dead
  **accepted** connection on the other (`handle_connection` + `InboundReentryEndpoint`). Peer ids
  are fresh each run, so the role flips run to run.
  **Say this precisely, because the spec pins the distinction and I got it wrong first.**
  `EXTENSION-SIGNALING` §6.5: *"The offerer governs negotiation direction, **never** who dials"* —
  both peers' ICE agents fire outbound, and the retracted `lower-dials/higher-listens` split is
  explicitly named as the non-traversing bug that framing causes. What the offerer rule decides is
  the **§7.4.1 initiator** (*"one role assignment, not two"*), i.e. who speaks HELLO first — and
  *that* is what puts one side on `spawn_reader_loop` and the other on `handle_connection`. The
  "one connection, reused both ways" half is `EXTENSION-NETWORK` §6.5.1b's duplex table and is
  **informative**, not a MUST. **A claim about the spec that you did not open the spec to make is
  the 09-04 review's §7 again** — the measurement was right and the explanation was invented. **When a gate is bimodal, make it
  print which mode it got** — the panel now runs on PASS too, with role needles (`reader:` /
  `terminating reader task` vs `remote disconnected (EOF)` / `reentry:`), because a run that does
  not say which role it took cannot tell a bimodal product from a flaky rig. `data channel
  closed: 2` for a single close is needle overlap (the EOF line embeds its own reason string) —
  **check whether your needles match each other before reading a count as an event count.**
  **The acceptor's teardown had the identical eviction**, plus no fast-fail of its own. Over a
  socket it appears to have one — the accept loop's writer task breaks on a failed `write_frame`
  and drops `resp_rx`, so `writer_tx.send` starts failing — but over a `MessagePort`
  `post_message` never fails, so the writer task never breaks and every send "succeeds" into a
  channel nobody reads. **That is why no native rig here had ever exhibited it, and why the
  neuter of that fix came back GREEN in the integration gate.** A neuter that passes has a third
  cause and this was it: the gate is sound, the neuter landed, and *the rig cannot produce the
  condition*. Recorded on the test rather than explained away, with a separate unit gate that
  **holds the receiver open on purpose** to reproduce a MessagePort — the same move as pairing a
  duplex read half with `io::sink()` on the dialer side. **Ask what your fixture's write half
  does when the far end is gone; if it errors, it is not the transport the bug lives on.**
  **And the first shape of the acceptor fix regressed something worse than the bug.** Keeping the
  endpoint registered with a flag beside its live sender took
  `a12_escalates_to_disconnected_after_the_a1_eviction` from **0.51 s to 30 s**: that endpoint's
  `resp_tx` clone is what keeps the accept loop's writer task parked, and that task owns the
  **socket's write half** — so an endpoint outliving its loop held a half-open socket open and its
  counterpart never saw EOF. The sender is now an `Option` the teardown takes, so releasing the
  handle and recording the close are one act. **A retained handle is a retained resource: when you
  extend something's lifetime, enumerate what it is holding.** Found by `make test`, not by
  reading — which is the argument for running the whole suite before believing a targeted gate.
- **AUDITED ACROSS THE COHORT — ALL THREE implementations leave an acceptor-only peer reading
  `connected` forever, by THREE DIFFERENT ROUTES, and the spec has a MUST-shaped hole at a site it
  never mentions (2026-09-08).** Packet:
  `docs/status/ROUTING-2026-09-08-B-arch-CONSOLIDATED-…` (one document; the `-A` plans doc is folded
  into it and was never delivered).
  **`entity-core-py` is audited now and it is NOT the same defect — it is the same end state with
  no arm to disarm.** Its acceptor writes `connected` (`peer.py:2923`, right after
  `_register_inbound_reentry`), and its demotion gates entirely on
  `remove_connection(peer_id, expected=…)`, which resolves `self._connections` — **the outbound pool,
  by object identity** — so a peer we never dialed cannot be demoted by any path; its keepalive is
  *"one loop task per pooled **outbound** connection"* by its own comment, exactly as Go's is. So Go
  has a `wasReentryBinding` arm its own teardown disarms, and py has no arm at all.
  **That trio is the argument, and it is stronger than the bug:** *a rule two impls get wrong is a
  coincidence; a rule all three get wrong by different mechanisms is a spec that does not say the
  thing.* **When you audit a cohort, do not stop at the seat that matches your own defect** — the
  seat that reaches the same broken state differently is what turns a bug report into a spec ask.
  **The composition is the finding, and both halves of it are individually correct.** §A1's
  no-clobber MUST (*demote only if the failed connection is still the currently-bound one*) and
  §5.4a's scope pin (*"an implementation that escalates on any unbound peer rather than on any
  `suspect` peer converts this rule into a new defect"*) combine so that **any teardown which
  unbinds a connection permanently silences liveness for that peer**: the demotion is refused
  because nothing is bound, the escalation is refused because the status is still `connected`,
  and nothing else writes. The spec never mentions connection teardown, so an ordinary
  clean-up-what-I-registered defer has no way to know it is standing on a normative seam.
  **Verified in `entity-core-go`'s source, not taken on report:** `(*Connection).serve`'s defer
  calls `unregisterInboundForReentry`, and `(*Peer).demotePeer` carries the identical
  `wasReentryBinding = p.inboundForReentry(peerID) == tcp` guard that the defer disarms. Worse
  there than here on two counts — `escalateUnboundSuspect` tests only the **outbound** pool, and
  `startKeepalive` is outbound-only by documented scope, so an acceptor-only binding has no
  keepalive loop at all. Reachable with no WebRTC: the validator-as-B, no-listener case Go's own
  comments cite. Its `wasReentryBinding` arm is, as far as a named search can tell, untested.
  **What Go already had is the argument for promoting our fix rather than keeping it local:** its
  `readerDone` latch is the same *primitive reports, seam acts* shape we landed as
  `reader_ended`, reached independently. **Two impls converging on a behaviour the spec only
  SHOULDs is when it should become a MUST.** (Reader-EOF fast-fail is implementation-defined —
  named searches for `EOF`, `half-open`, `read half`, `DEFAULT_REQUEST_TIMEOUT` over the arch
  specs return nothing; the obligation is V7 §6.11's *informative* teardown SHOULD.)
  **And a spec-hygiene finding that takes ten seconds to check:** the Amendment 12 proposal's
  header says *"FOLDED IN FULL… All Amendment 12 spec deltas are now in `EXTENSION-NETWORK`"*
  while its own fold table one screen down says only the §A1/§5.4 **join** landed.
  `mark_connection_closed`, `seam discipline`, `currently-bound`, `no-clobber` → **zero hits
  combined** in `EXTENSION-NETWORK.md`. So the two rules an implementer must compose live in two
  documents, one filed under `proposals/implemented/` where it reads as history. §5.4a's own
  root-cause note already named this — *"§A1 lives only in the proposal, §5.4 lives here, nothing
  owned the composition"* — and it produced a second defect of the same shape anyway. **A header
  that overclaims a fold is worse than one admitting a partial: it is the reason nobody re-reads
  the composition.** Same family as *recording a gap is what makes it look handled*.
  **The bound this leaves, and it is the operator-facing half: on the acceptor side liveness is
  corrected by a DISPATCH, not by the teardown.** A peer that accepts a connection, never
  dispatches back, and whose counterpart vanishes still renders `connected`. That was true before
  too — the change makes it *correctable* rather than permanent. The browser always dispatches
  (the chat poll, the wake probe), which is why our gate closes; **a server-role peer that only
  ever answers does not.** The responder writes `connected` at the AUTHENTICATE grant and the §A3
  slice gives it no way to retract it: both demotion paths need a peer we dispatch at or keep
  alive, and an acceptor-only binding is neither. Do not describe the acceptor side as covered.
- **COMPARING TWO e2e RUNS ON DIFFERENT GRID STATES IS NOT A COMPARISON — 2026-09-08, and it
  produced a confident wrong conclusion in one step.** `e2e-webrtc-chat`'s §11.5 deposit counter
  read **12/side (FAIL)** with a change in and **4/side (PASS)** with it stashed, which reads as
  proof. It was not: those two runs sat at different points in a **four-gate sequence on one
  Selenium node**, and the metric degrades as the node ages. Disabling **both** new handlers still
  reproduced 8/side — *that* is what caught the error, and it is the move to reach for: **neuter
  your own change entirely; if the symptom survives, the symptom is not yours.** On a fresh
  `make e2e-grid` per run the change measured **4 / 4 / 4, PASS**.
  AGENTS.md already said *"`make e2e-grid` before you run the suite"* and *"always **replaces** the
  node, so a run starts on a cold browser"* — written about the unfiltered suite, and I read it as
  advice for long runs rather than as a precondition for **any** comparison. **A/B on a shared
  mutable rig requires the rig reset between arms, not just before the first one.** Deposit counts
  in particular are a *ratio* metric against an O(1) bound of 8, so a stale node walks them
  straight through the threshold.
- **DURATION CANNOT SEPARATE A SUSPEND FROM A WEDGE — but WHICH THREAD STOPPED can, and the
  design doc that said otherwise had not read the worker (2026-09-08).** `watchdog_policy` dropped
  any gap over 60 s as *"must be sleep"*, and the standing write-up called that a hole: *"a device
  that wakes and then wedges produces one long gap."* **It does not.** The watcher lives off the
  main thread and resets its own clock after every report (`last = Date.now()`), so while the main
  thread is wedged it keeps ticking and emits a **stream of threshold-sized reports** — a single
  gap far larger than the threshold can only mean the *watcher itself* stopped, i.e. the whole
  process was suspended. The heuristic was sound; what was wrong was that it threw the conclusion
  away. **Read what GENERATES the input before you call a heuristic on it wrong** — the claim was
  reasoned from durations alone and would have sent a session to fix a non-defect.
  What was genuinely broken is smaller and is AP40: three guards collapsed into one `bool` and one
  log line reading *"tab backgrounded / device sleep — not a real freeze"*, naming three causes and
  committing to none, so the Event Log could not answer the one question an *"it froze after
  waking"* report turns on. Now four outcomes (`Backgrounded | ResumeRace | EnvironmentGap |
  Freeze`), each with its own sentence, and `is_freeze()` is spelled **positively** so a fifth
  environmental cause cannot silently start rendering as a stall (`AppServerView::is_serving`'s
  bug, one subsystem over). `a_wedged_main_thread_still_reports_because_its_gaps_are_threshold_sized`
  pins the reasoning above as a test, so a future author cannot re-derive the wrong version.
- **THE DESKTOP'S RENDEZVOUS AND APP SERVER DEFAULT **ON** (2026-09-07, operator's call) —
  `persistence::{DEFAULT_SIGNALING_NODE, DEFAULT_APP_SERVER}`.** A fresh install was neither, so
  the whole zero-config LAN path (*walk over, type the URL, you are paired before the app boots*)
  existed in full and was switched off behind two System Overview rows you had to already know
  about. **`DEFAULT_PORT_MAPPING` deliberately did NOT move, and that is what made this safe:**
  its neighbours bind on this LAN, that one reaches the internet. Do not fold them together later
  because they read alike in a struct. Three things it cost to get right: **(1)** the defaults are
  **one expression** consumed by `read_config`'s absent-key arm *and* both `lib.rs` construction
  sites, because a peer that serves now and not after a restart is the drift (AP44); **(2)** an
  **explicit `false` is preserved** — `set_peer_flag` writes the key, so a user who turned either
  off keeps it off; **(3)** *absent key* and *unreadable document* stay apart — `PeerConfigFile::
  default()` is still fail-closed on all three, because a document we could not read is not
  permission to open a listener (AP40, one tier down from `deployment_config::read_document`).
  **The gap this change created in itself, and how it was found:** the setup hook's app-server
  restore runs **before** any peer is auto-provisioned, so on a *genuinely first* launch the
  persisted set is empty and the on-by-default server would not have served until the **second**
  launch — the launch nobody verifying a fresh install performs. Found by asking *what does this
  default reach*, not by a red test: the native tests cover the flag's **value** and are blind to
  **who reads it**. Fixed with `ensure_spa_server`, called from the setup hook *and*
  `ensure_system_backend`. Gates: `a_freshly_created_peer_reads_back_the_shipped_service_defaults`
  (falsified — emit `signaling_node = false` from `write_default_config` and it reds; it asserts
  the **round trip**, since `assert_eq!(x, CONST)` on the next line measures the author's memory)
  and `an_explicit_off_outranks_the_new_on_by_default` (falsified). **Verified live, cold:**
  `rm -rf .tauri-home && make tauri-run` → *"SERVES §6.5 rendezvous at ws://…"* + *"app-server:
  serving at http://…"*, zero toggles, plus the late arm firing against its own new backend.
- **THE APP SERVER MOVED OFF ITS PORT AND THE THING THAT TOOK IT WAS THIS SAME APP, OLDER —
  2026-09-07, and the diagnosis cost two sessions.** `app_server::start` fell back to an ephemeral
  port when 8081 was taken, on the reasoning (in its own doc comment) that *"a developer box
  routinely has `make serve` already holding 8081, and refusing to start is a worse answer than
  starting somewhere the caller can read back."* **Both halves are false for this service.** The
  caller *cannot* read it back — the whole purpose is that a person walks to **another device** and
  types the URL, where they type the port they always type. And what held 8081 was a leftover
  `make serve DIST=dist-site` container from three days earlier, so the other device got **the same
  UI running older code** — predating both `71c7d21` (the meet message) and `f9bb540` (the late
  arm), which is exactly why it presented as *"chat regressed"* and *"it says I'm not on my main
  peer"* on a single-peer profile where that message is unreachable in current code.
  **A wrong answer that renders correctly is worse than a connection refused.** It now hard-fails
  and the error **names the port**; `port: 0` still means *any free port*, because a caller asking
  is not a fallback deciding.
  **The invisibility had three layers and no single one was the bug.** (1) The warning was a
  `log::warn!` to stdout, which the GUI never shows. (2) `AppServerView` had **three** states, so a
  failed bind graded as **`Off`** — indistinguishable from *"you switched it off"*, the AP40
  collapse one tier down from `read_document`'s. There is a fourth state now, `Failed { detail }`,
  carrying the backend's own sentence rather than a localized *"could not start"* that would drop
  the port number, i.e. the only actionable part. (3) **`make serve` runs `--network host`, so
  `podman ps` shows an EMPTY ports column** — the operator looked for a container on 8081 and the
  tool correctly told them nothing. *When someone says "there's nothing on that port", check how
  they looked before you disagree*: `ss -ltnp` and a `curl` of the build stamp settle it in two
  commands, and `curl -s <url> | grep entity-build` is the one that names *which build* is
  answering.
  **The bug the fix itself introduced, caught by its own new test:** `is_serving()` was spelled
  `!matches!(self, Off)`, so the moment a fourth variant existed it classified as *serving* and the
  row would have shown the toggle ON for a server that is not running — leaving *switch it off* as
  the only way to retry. **State a predicate positively when it drives a control**; a negative
  spelling silently absorbs every variant added later. Gates:
  `a_taken_port_fails_loudly_and_names_the_port` (falsified — restoring the fallback reds it,
  reporting the port it bound instead), `port_zero_is_still_an_explicit_request_for_any_port`,
  `a_server_that_could_not_start_is_not_reported_as_switched_off`,
  `a_live_socket_outranks_a_stale_recorded_failure` (a recovered retry must not leave the row stuck
  on a fault that is over) and `an_empty_error_is_the_same_absence_as_no_error`.
  **Standing check when a report says "it used to work":** confirm which build the reporter is
  actually running before tracing code. `make tauri-run` builds the current checkout correctly —
  but only *that* checkout, and only onto a port it can get.
- **THE RECOVERY CONSOLE THREW AWAY THE HALF OF ITS REPORT THAT DOES NOT NEED THE ORIGIN — AP36,
  in the surface built to diagnose AP36's family (fixed 2026-09-07).** `index.html`'s routing card
  opened `if (!doc) return setEmpty(…)` — an early return on the **acquisition** — so an origin
  that could not be reached discarded *what this profile believes* **and the entire Retired
  publishers card**, at exactly the moment a stranded visitor needs them, since an unreachable
  origin is a normal condition for one. It also made the verdict's own `!home` arm (*"the domain
  did not answer, so there is nothing to compare it against"*) **unreachable**: someone wrote that
  branch and an early return above it meant nobody ever saw it. **Put the guard on the DECISION,
  never on the acquisition** — and the probe is now **three** states, not two (served /
  answered-with-a-status / did not answer), because a 502 is not a deployer choosing to serve no
  config, the same AP40 split `deployment_config::read_document` carries one tier down. Found by
  building a gate that needed the console to work with no document, not by reading the console.
- **THE ADOPT AND THE UN-NAME ARE A PAIR — §1.1f item 3, shipped 2026-09-07
  (`origins::unname_withdrawn_origins`).** `adopt_deployment_origin` only ever **adds**, so a peer a
  domain stopped hosting kept a registered origin that 404s on every visit — in the browse-all
  roster, in the retry ladder, and in Doctor's `fetch-failure-by-peer`. *Un-name before you remove*,
  run in the other direction from the `--prune` finding that earned it.
  **The decision is pure (`withdrawn_rows`) and three of its four rules are refusals**, which is
  where all the risk in a delete path lives: an **empty declared set withdraws nothing**, a
  **`user`-marked row** is never withdrawn, and the **resolved home peer's row** is never withdrawn.
  **The empty-set guard is what decides whether this is safe to ship.** `DeploymentConfig` parses
  `origins` into a `BTreeMap`, so an **absent** `origins` key and an explicit `origins: {}` arrive
  **identically** — the AP40 collapse, one field over from the one `superseded` avoids by never
  emitting an empty key. A minimal document (`{"surface":"site"}`) names no origins and `dist/` has
  no document at all, so sweeping on an empty set would empty the registry of every profile that
  booted against either. Stated bound: **a deployer cannot express "I host nobody"**, and that is
  the direction we can afford to be wrong in.
  **A superseded row needs no special case, and that is what a tidy version gets wrong.** Through a
  re-key transition the retired peer's entry is deliberately still *in* `origins`
  (`HomeClaim::Takes`), so rule 3 keeps it and `apply_supersession`'s carry-forward arm still has
  the row it carries. The distinction from that function's *"availability lost to hygiene"* warning:
  **`apply_supersession` acts on an inference with no document in hand; this acts on the live
  document explicitly not naming the peer.**
  **We remove the NAME, never the bytes** — the withdrawn peer's cached content is untouched (D24,
  no export path), and un-naming is reversible by the next publish (E1, `Adoption::Seeded`).
  **Assert the LISTING, not what `remove_and_wait` returned** — falsified by a report-only
  implementation, which the return-value assertion passes. The gate that matters is the consequence
  one, `a_withdrawn_publisher_stops_serving_apps_to_a_returning_profile`: AP54's twin, same
  warmed-catalog precondition and same reason — without it `app_source`'s first loop finds nothing,
  the fallback takes *"the first foreign origin"*, and sort order decides.
  **Register through the ADOPT path in any test, never `set_origin`** — that one marks `user`, which
  is exempt, so a fixture built on it makes the sweep a no-op and the gate vacuous.
  **Measured, and it bounds what a browser gate could ever assert: the origin registry has no
  user-visible readout.** All seven `list_origins` consumers either subscribe a tree prefix or pick
  a fetch target; nothing renders a row. That is most of why the 2026-09-05 incident was invisible.
  **The wiring is a census, not a structure** — `tests/origin_reconcile_census.rs` asserts both
  calls are in `src/app.rs` and that the sweep comes **after** the adopt, ships its own two-way
  falsifier, and was falsified against the real file. The better fix is one `reconcile` function
  doing both halves (AP44); not taken because the adopt loop's `expand_origin` is WASM-only and
  three e2e gates key on that loop's exact log lines. Design: §1.1f-b.
- **`/entity-deployment.json` is DOMAIN-managed and describes the whole domain — the publisher
  used to clobber it down to the last peer (fixed 2026-09-03).** `emit_deployment_config` built a
  fresh single-entry `origins` map and `fs::write`'d over the file, so publishing a second peer
  under its own `--prefix` — the multi-peer-at-one-origin shape the tooling has supported since
  prefixes existed, and which `DESIGN-DEPLOYMENT-GENERATIONS` §7 specifies as *"names peers, their
  prefixes, their active generations"* — dropped the first peer's origin **and** moved `home_site`
  by publish order. **And that flip wrote a FALSE supersession**: `decide_home` sees
  `AdoptDeclared`, boot persists `alpha → beta` against a peer that is alive and serving, and
  revalidation *keeps* it because the document does agree beta is home. F2's brick through a
  different door. It merges now: **the home publish owns the domain-level fields; a secondary
  publish contributes only its `origins` entry**, `home_claim` is the pure four-way decision, and
  `--set-home` is the deliberate act that moves a home. An unreadable existing document is a hard
  stop — starting fresh there performs the exact clobber being fixed.
  **The methodological half, and it is the reason this was missed twice.** The first pass sampled
  the emitted documents, found one origin in each of six, and concluded multi-peer was unused and
  therefore not a live defect. **Backwards:** every document had one origin *because the emitter
  could not write two*, so the sample that looked like evidence of disuse was the defect's own
  fingerprint. **When the question is what a tool can express, read the tool — a survey of its
  output cannot distinguish "nobody asked for this" from "it cannot do this".** Measured by
  publishing two peers and looking, which took four minutes and overturned the write-up.
  Gates: `two_peers_on_one_domain_both_survive_in_the_document`,
  `a_second_peer_defers_to_the_domains_existing_home`,
  `an_unreadable_domain_document_is_not_overwritten`. The re-key fixtures pass `--set-home`, which
  is semantics and not a workaround — a re-key *is* a deliberate home move, and without it the
  fixture would emit a domain that never re-keyed.
- **Several publishers may share ONE hosting scope, and until 2026-09-03 publishing the second one
  DESTROYED the first (AP52/AP53). Read this before touching `publish`'s clean or `DirFetcher`.**
  The typical topology is peers sharing an origin with their **trees** telling them apart —
  `--prefix` is the *hosting* scope, `peer_id` is the *authority* scope, and the layout is
  peer-scoped at `{peer}/…` and `sites/{peer}/…` with `content/` a **shared, content-addressed
  store**. Two things were scoped to the wrong level:
  **(1) The clean deleted `{base}/sites` and `{base}/content`** — the container, not the peer — so
  a second publish removed the first peer's signature blob and its whole projection.
  `publish --verify` on the first: *"1 BROKEN entry — this tree is not safe to serve"*, *"a pinned
  consumer resolves NOTHING from this tree."* Its tree directory survived, which is what made it
  quiet. Now `sites/{peer_id}`, and **`content/` is never deleted when a sibling publisher is
  present** — accumulating orphans (which `--verify` lists) is recoverable; deleting another
  publisher's signature is not. §7's origin-wide keep-set is the real answer.
  **(2) `transport-profile` is ONE artifact per hosting scope with PER-PEER contents, and
  `DirFetcher` followed it blindly** — *"in order of authority: what the publisher advertised, then
  our own convention"* — so reading peer A followed peer B's layout and looked for A's manifest
  where B's lives. **This was filed as cosmetic and a gate written for (1) failed on it**: two
  independent defects, one symptom, against a tree that was by then completely intact. The rule
  now: **an advertised layout that DECLARES a different peer is not authority for this one**
  (`profile_peer_id`), and **absence is trusted** — a conformant publisher need not emit `peer_id`,
  so only a positive mismatch disqualifies, or we would stop being able to read core-go.
  **Still bounded, publisher side:** one `transport-profile` URL per scope, so an origin serving
  several publishers advertises one of them to a cold consumer; `sites/index.html` likewise names
  only the last publisher; and content is **duplicated per prefix** (measured 17 of 19 blobs).
  Publishing prints all three when a sibling is present. Gates:
  `a_second_publisher_at_one_origin_does_not_break_the_first` (asserts through `publish --verify`,
  because **a file count passes the entire time the tree is broken** — both peers publish the same
  set) and `a_profile_names_the_peer_it_is_about_and_silence_is_not_a_mismatch`.
  Audit: `docs/plans/AUDIT-MULTI-PEER-HOSTING-AND-THE-HOME-SITE-2026-09-03.md`.
  **And the fix removed an isolation property 24 e2e tests were silently relying on.** `publish`'s
  destructive clean had been doing double duty: `emit_deployment_config_fixture` publishes into
  `dist/`, every isolated scenario stages itself with `link_tree("dist", &root)` — which
  **hardlinks that `/entity-deployment.json` in** — and once the document merges rather than
  clobbers, each fixture *deferred* to the previous scenario's home and emitted a domain that was
  not the one under test. **41 passed / 24 failed unfiltered, and every one of the 24 green when
  run alone.** *Filtered green + unfiltered red = shared state, not a product defect.* Fixed by
  passing **`--set-home` in every fixture emitter** — semantics, not a workaround: each one defines
  the domain its scenario boots against. **When you make a destructive operation safe, ask what was
  depending on the destruction** — nothing had ever declared that a `rm -rf` was what isolated the
  fixtures.
- **WE HOST THE TREE. A site is ONE L5 application projected out of it, and the HTML is a
  projection for browsers only — read this before designing anything that publishes.** A domain is
  a way to host a tree; what is in the tree may be sites, apps, compute programs, a follow feed,
  comms, relay notes or backups, and a consumer that is not a browser (workbench-go) goes to the
  **tree**, never to the HTML. `EXTENSION-NETWORK` §6.5.3 says it in those terms — *"has published
  its **tree + content**… the bytes-on-wire ARE entity-encoded"* — and **Amendment 9 already ruled
  the layering**: the reserved-word table is explicitly extensible, each L5 convention registers its
  own projection prefix, and *"NETWORK does NOT enumerate the registered extensions."*
  **What is already right, and a feed inherits all of it for free:** the consumer is generic
  (`SignedSession::resolve(src, relative_key)` / `enumerate(prefix)` walk **arbitrary peer-relative
  tree keys**); the endpoint describes a tree, not a site (`PublishLayout`'s five fields name no
  site); **`publish --verify` and the clean are already tree-shaped** (`run_verify` walks
  `base.join(peer_id)` — every `.bin` under the peer, whatever wrote it); and `apps` is deliberately
  **not** a reserved word, living at `/{peer}/apps/{set}/…` sibling to `/{peer}/sites/{site}/…`
  inside the tree. **Copy the `apps` shape, not the `sites` shape**, for anything new.
  **The one thing that is backwards, and it is publisher-side only: what ENTERS the projection is a
  hardcoded enumeration of two L5 conventions, not a policy over the tree.** `publish` is
  `emit_owned_sites(sites)` + `for set in app_sets { emit_app_set }` + `root.finish()`, and
  `RootProjector`'s doc states the consequence — *"the root commits to the bytes we projected, not
  to the tree we read from… never over the source peer's whole tree, which holds keys and app state
  a publish must not commit to."* **That reason is sound and must survive any fix** — so the real
  requirement is a *publication policy* (which subgraphs are public), and today there is none: a
  grep for `publishable` finds a comment. Adding follows touches five mechanical per-axis sites
  (`resolve_publish_source`, the emit block, `run_plan`, the `warn_replaced_*`/`projected_*` pair,
  the `http_poll` URL builders) — AP44 reaching a third subsystem. **Fix it structurally WITH
  follows, not speculatively before.** (`ForeignArtifact` staying a hand-edited closed enum is
  **correct** — that is D24 forcing the currency question per kind.)
  **And the gap that is larger than it looks: `publish` cannot publish a peer's tree.**
  `resolve_publish_source` builds a **fresh in-memory peer** each run and seeds it (demo set, or
  `--ingest` of a `render/` dir); the *keypair* is durable, the *content* is assembled at publish
  time. Every CLI flag is site- or app-shaped — there is **no verb naming a peer and no flag naming
  a subtree**. Its sibling: §6.5.6 **live serving mode is unimplemented** — `src-tauri`'s
  `app_server` looks paths up by exact key in the embedded SPA map and serves **no** tree routes, so
  the one component with a real durable tree cannot expose it. Both are documented deferrals, not
  regressions; F3 is the cheaper one.
  Full review, with the citations: `docs/plans/REVIEW-2026-09-03-b-THE-TREE-IS-THE-PRODUCT-AND-THE-SITE-IS-ONE-PROJECTION.md`.
  **REPUBLICATION IS BYTE-EXACT AND AUTHORSHIP IS ROOT-ANCHORED — `tests/mirror_byte_fidelity.rs`,
  in `make test`.** A real store re-serves another peer's exact encoding: measured through wire decode →
  `MemoryContentStore` put/get → the emit call `RootProjector::finish` uses, with a **deliberately
  non-canonical** fixture (`{"n":1}` with the int in non-minimal `uint8` form) because round-tripping
  bytes our own encoder produced proves nothing. Falsified both ways. It holds structurally, not by luck:
  `ecf_for_hash` *"embeds `data` bytes directly without re-encoding, preserving byte fidelity (matching
  Go's approach)"* and `Entity` hashes over verbatim `data`. **Trap: `impl PartialEq for Entity` compares
  `content_hash` ONLY** — the wrong granularity for any byte-fidelity question; compare `data`.
  **But an `Entity` carries NO signer.** `SignedSession::resolve` is root-anchored — an entity is
  authentic because it is reachable from its author's signed root — so a lifted entry has **integrity
  without authorship**, and per-entry signatures do not exist to be "kept". Anything mirroring foreign
  bytes must carry an **inclusion proof** (the author's root + the trie path, cheap because HAMT nodes
  dedup) or accept that authorship needs the author's origin. Run this gate for any change to the emit
  path, the store round-trip, or a mirror/republication surface.
  **WHAT THAT GATE DOES NOT ESTABLISH, AND A BLOCKER WAS NEARLY DERIVED FROM IT AGAIN ON
  2026-09-07.** It measures **a capability of the store**, not a requirement the consumer imposes —
  and the product's HTTP ingest path does **not** preserve a foreign encoding.
  `http_poll::verify_and_decode` re-encodes a fetched entity's `data` with `to_ecf` and *then*
  validates the hash, so a publisher who hashed canonically gets their canonical bytes stored at the
  address their root commits to, and one who hashed non-canonical bytes raw is refused with
  `HashMismatch`. **No non-canonical entity enters the store over HTTP, either way.**
  **That is conformant, not a shortcut.** `ENTITY-CBOR-ENCODING` §5.4 blesses **both** mechanisms —
  store-and-forward the original bytes (steps 3–4, what the gate measures) *or* carry the validated
  hash and re-encode canonically on receipt, which is the arm we are on; §9 states the same rule from
  the other side (*"Always re-encode to ECF before hashing. Never hash received wire bytes directly,
  as they may be valid but non-canonical."*).
  **So the handoff's Q1 — "the L1 `put` wire is lossy for bytes we did not author" — has a weaker
  premise than it was written with.** `build_put_params` does re-encode (`data` goes as a decoded
  `Value`, re-encoded peer-side at `core/tree/src/lib.rs`), but it is lossy for **exactly the set of
  entities we already cannot ingest**, and option (c), *canonicalize on ingest*, was written down as
  "probably a non-starter, breaks author-anchored verification" when it is **what we already do and
  what the spec permits**. Note also the lossy hop is narrower than "L1": `WireEntity` carries `data`
  as **raw bytes**, so the app→worker channel is byte-faithful, as is `WriterHandle::Direct`'s L0 put.
  **Corollary that bounds every mirror design: a publisher who hashed their own non-canonical bytes
  is unmirrorable by anyone**, because their root commits to a hash no conformant impl reproduces.
  That is the spec working, not a gap. **Still open and routed, not closed here:** §5.4 clause (a)
  requires re-encode-**and-compare against the received bytes** where we compare hashes, and the
  appendix requires a re-encode arm to run the `encode_equal` round-trip vectors — an `entity_ecf`
  obligation nobody here has verified is run.
  **A SITE AND A FEED ARE THE SAME MECHANISM WITH A DIFFERENT DATA MODEL — do not re-derive a blocker
  here, one was invented on 2026-09-04 and withdrawn the same day.** Entities at a peer-scoped tree path,
  projected to static files, committed to by a signed root; `RootProjector::record(peer, subpath,
  entity)` is already generic over what the entities mean. So arch's *"publish a feed, follow feeds is
  easy"* is right **here** too, and F1's residue is a **reader plus an emitter** — the five mechanical
  call sites the review already scoped, best spent on the `(tree_prefix, reader)` registration table
  (AP44) rather than a third arm.
  **And THE TREE IS A UNIVERSAL NAMESPACE: a foreign peer's entities live in MY tree under THAT peer's
  own segment**, at their natural path — `foreign_cache::store_path` → `paths::manifest_path(foreign,
  site)` = `/{foreign}/sites/…`, and `paths.rs:112` states it (*"the universal tree carries the
  partition… it never filters by peer"*). `emit_owned_sites` is already multi-peer (`OwnedSite.peer_id`)
  and `RootProjector::record` skips foreign peers **from the signed root only** — *"a publish can project
  more than one peer's subgraph"* — which is exactly the mirror shape: served at the author's path,
  verified against the author's signature, not claimed by my root. **So mirroring needs no new storage
  class, and `author == namespace` holds trivially for a republished entry.** The withdrawn finding
  claimed the opposite; it read `record`'s skip as a refusal while the next sentence of the same doc
  comment says otherwise.
  **The one genuinely absent verb, at its real size:** `resolve_publish_source` assembles into a scratch
  peer holding the **durable keypair** (`--ingest <render dir>` or the demo seed) — `disk → tree →
  project → sign`, which is what devops publishes with. There is **no verb reading a long-lived native
  store**; that matters for publishing a tree out of Tori (F3's other side) and does **not** block a
  feed. Cross-check, five surviving findings routed to arch, and the correction record:
  `docs/plans/REVIEW-2026-09-04-THE-SOCIAL-TIER-CROSS-CHECKED-AGAINST-THE-BROWSER.md` — whose §7 is the
  transferable half: **choosing the symbol after the claim produces a review that looks checked and is
  not; if nothing you opened contradicted you, you did not run a check.**
  **Naming warning:** the transport lives in the wrong namespace — `http_poll.rs`, `signed_root.rs`,
  `signed_fetch.rs`, `publish_layout.rs`, `origins.rs`, `foreign_cache.rs` are all under
  `src/content_site/` and **none of them is a site concern**. Nothing depends on it; it is simply
  what makes the next author reach for a site-shaped answer. Split it as part of the first change
  that proves the boundary, never as a standalone rename.
- **A `home_site` is a `(peer, site, page)` triple — NOT the domain, the root, or a URL.**
  `SiteRef { peer_id, id, loc }`: which publisher, which of their sites, which page (empty `loc` =
  the manifest root; empty `peer_id` = the documented sentinel for *this profile's own peer*). It
  answers *where does this deployment open*, and it is **resolved, not addressed** —
  `origins[peer_id]` supplies the origin and the URL is assembled. So moving where a publisher is
  hosted does not touch `home_site` at all; they are orthogonal by design. Two consequences:
  **a domain has exactly one home**, so with several publishers exactly one holds it and that is a
  *deployment* decision rather than a property of any publish; and **moving it between publishers
  is not cosmetic** — for a returning profile the old home peer is recorded as *retired* and every
  stored reference to it is rewritten, which is right for a re-key and wrong for merely adding a
  second publisher. Hence `--set-home` / `SET_HOME=1`.
- **The three tiers of reference, and only one is stable — read this before designing anything
  that stores a peer id.** Every fetch is `{origin}/{peer_id}/…` with the **hosting prefix riding
  inside the origin** (`deploy_origin` → `{live}/{prefix}` or `/{prefix}`), so the two mutable
  things move independently: **(1) Domain** — `/entity-deployment.json` at a fixed path; the path
  is the stable thing, the deployer manages it, and it *is* the current truth, re-read every warm
  boot. **(2) Name** — a registry binding; the *name* is stable, and the binding carries **both**
  the new peer id and the new origin (`arch D10`: every peer-issued binding must carry one), so a
  re-key and a CDN move are the same act to a consumer resolving by name. **(3) Raw peer id** —
  stable in nothing; it is in every URL and a re-key changes all of them.
  So: **a prefix/CDN move is already multi-peer-correct** (it changes `origins[peer]`, adopted
  per declared peer on every warm boot); **a re-key is the half with the gap**, and a raw peer id
  is the weakest handle we have. Prefer the document or a name wherever one is available.
- **The refresh ledger is fed by `foreign_cache::ensure_current`, NOT by its callers.** It was fed
  by **one of three** consumers — the Apps window — so the health section's check 2, documented as
  *"the signature from incident A"*, could not see incident A's own fetches (the boot content-site
  sweep and the discovery sweep recorded nothing), and its `Agrees` line claimed *"every publisher
  asked has served something"* over a set that was not the set that was asked. Recording at the
  chokepoint is a **witness, not a notification** (AP44): `tools/foreign-cache-lint.sh` already
  makes `ensure_current` the only legal way to fetch a foreign artifact, so a consumer added
  tomorrow is covered without knowing the ledger exists. Do not add a `refresh_ledger::record`
  call to a new fetch path — if you feel the need to, the fetch is bypassing `ensure_current` and
  that is the bug.
- **A retry ladder branches on `PollError::is_terminal`, never on a status or a variant match of
  its own.** *Do not retry a 404* was the design's R3 and stayed open for weeks while the Apps
  window's ladder matched `Currency::Unavailable(_)` and consulted the error only **after** five
  attempts — so a set the publisher had withheld cost ~9 s of waiting on an answer that arrived in
  the first 200 ms, and the health finding that reports it could not appear until the ladder ended.
  The ruling was never missing: `PollError::NotFound`'s doc comment already argued terminality
  (`EXTENSION-TREE` §3.3a) and named the trap — *"a 5xx or a dropped connection is not this"*.
  `is_terminal()` is that paragraph made executable, with an exhaustive match so a sixth variant
  gets a decision rather than a default, and a census
  (`only_the_answer_an_origin_chooses_is_terminal`) that asserts the count.
  **Both directions cost something, which is why the conservative line is 404/410 and nothing
  weaker:** calling a transient fault terminal turns a CDN hiccup into a permanently missing app,
  and a truncated body arrives as `Decode` while a proxy-mangled one arrives as `HashMismatch` —
  transport faults wearing a content fault's name.
  **The ladder's decision is pure and native (`ladder_step` → `Retry | Withheld | Exhausted`)**,
  because its only caller is a `spawn_local` inside a `cfg(wasm32)` block that no native test can
  reach — and *"~9 s of coverage for a blip"* is exactly the kind of claim that lives in a comment
  and drifts (`the_ladder_costs_what_its_comment_says` computes it). Three words, not two: the two
  arms that stop differ in what they license you to say, and *reopen the window to retry* is wrong
  advice for a publisher that already answered (AP40). Falsified both ways at
  `PollError::is_terminal`.
  Note what is **not** changed: `resolver.rs`'s per-site failure backoff still re-asks a withdrawn
  site after its backoff, deliberately — it renders the error immediately, costs the user no wait,
  and a site that returns should be picked up.
- **`poll_json` returns `Ok(last_value)` on timeout — it does NOT error, so `.await?` is not a
  check (AP47).** That is the right design for its common caller (poll, *then* assert), and a
  trap for the other one: `poll_json(..).await.map_err(|e| "X never happened")?` describes a
  condition it cannot detect. **Assert on the value it returned.** Found because a neuter written
  to falsify a new gate came back **green** — and the first explanation to hand (a mis-anchored
  `replace` that patched an inner call site with identical text) was *also* true. **A neuter that
  passes has two possible causes and you owe both:** the gate does not measure it, or the neuter
  did not land. Check the served bytes before concluding either.
- **The boot surface stays up until the frame loop arms — AP46.** `start()` used to hide
  `#loading` before peer construction, i.e. before `boot_load`'s ~14 awaits, and with
  `boot_fast_paint` `DISABLED_FOR_CONSOLIDATION` **nothing painted in its place** — so the page
  was blank for the whole application-tier boot (bounded worst case tens of seconds) and the
  always-visible *"Open System Recovery"* hatch, which lives inside that same div, was gone at
  the moment it was needed. **Take a fallback down on *the replacement is LIVE*, never *it has
  STARTED*.** `boot_progress::armed()` is the structural half and has exactly ONE call site,
  after the first `requestAnimationFrame`, so a boot that dies earlier keeps the surface for
  free rather than via a handler someone has to remember (AP44); `boot_progress::step()` names
  the running step and is **best-effort by design** — a missing call costs one line in a bug
  report and nothing else. Its labels are `// i18n-ignore`: this is the same tier as the L1
  recovery console, and `index.html` carries no i18n at all. Gate:
  `a_stalled_boot_shows_the_boot_surface_instead_of_a_blank_page` — run on the **black-hole
  rig**, because there the stall is load-bearing and ~3 s wide while a healthy boot makes
  catching the surface a race. Falsified both ways.
  **It does NOT make an app-tier step non-fatal.** That is boot-B-1's two-phase boot, and
  `AUDIT-BOOT-PATH-2026-08-27.md` **§4a** names the constraint: `boot_load` takes `&mut self`
  while the rAF closure `try_borrow_mut()`s the same cell every frame, so spawning it behind an
  armed loop reproduces the blank page with a `FRAME SKIP` line under it.
  **The way out is `DispatchHandle`, NOT a shareable `Peers` — §4a's first answer was mine and
  was wrong, corrected the same day.** `DispatchHandle` is already cloneable, arm-agnostic and
  transport-owning, built for exactly *"several L1 calls in a row from a spawned task"*; take
  the handles in Phase 1, then await holding no `Ref<EntityApp>`. The *"single-peer with no
  read"* line from the B-3 handoff describes today's API surface only — `PeerContext` already
  has capability-checked async `get`/`put`/`list`, so it is a pass-through to add, not a router
  rewrite. **Observability and non-fatality are separable; only the first was cheap.**
- **The boot is TWO PHASES, the split point is the one NETWORK read, and the deferred order is
  the DEFAULT since 2026-09-02 — there is no flicker and no trade left to pick.** `boot_load` is
  phase 1 (local reads only: roster, window index, durable session config, supersessions) and
  returns a `BootPlan`; `EntityApp::boot_phase2` is everything that depends on
  `/entity-deployment.json`, and it runs spawned, behind a live frame loop. `?boot=inline`
  restores the old order as an escape hatch and as the gate's control; nothing in the product
  sets it.
  **The flicker was real and it was NOT inherent to the ordering — it was one signal doing two
  jobs.** `boot_progress::armed()` meant *the rAF loop is live* AND *take the boot surface down*,
  so deferring phase 2 necessarily painted a surface phase 2 might then change. Those are
  **`frame_loop_live()` and `surface_down(reason)`** now. The frame loop goes live early — that
  is the entire anti-brick property, and it is bought by *where the loop is armed*, never by
  *when the surface comes down* — and the boot surface stays up until phase 2 hands the page
  over. **The user sees exactly one transition, the same one they always saw.**
  Why the surface genuinely cannot come down earlier: the startup surface is decided in phase 2,
  behind the document *and* the supersession adoption, which
  `rekeyed_domain_heals_on_next_boot_window_surface` already established as a **data dependency**
  when a first cut put the spawn in phase 1 and reproduced the re-key incident's symptom.
  Painting before that is guessing.
  **The hold is bounded in `boot_progress` itself, and that failsafe IS the guarantee (AP44).**
  Holding until phase 2 reports back re-creates the brick if phase 2 never reports back, so
  `frame_loop_live()` — one call site — arms a `HOLD_FAILSAFE_MS` (30 s) timer that takes the
  surface down regardless. **30 s is deliberately ABOVE phase 2's own worst case** (one 3 s D23
  document deadline + up to four 5 s origin seeds ≈ 23 s); a failsafe *under* that number fires
  on a boot that is merely slow and produces the exact flicker this shape avoids. `?boothold=<ms>`
  is a test affordance nothing in the product sets — same shape and same reason as `?bootstall=`,
  since reaching the branch honestly means a 30 s gate.
  **The hand-over is an RAII guard, not a line at the end** — `boot_progress::HandOver`, armed at
  *did not complete* and promoted by `completed()`. A trailing call is reached only when phase 2
  returns normally, and the paths that most need the page handed over are a panic unwinding out of
  an await and an early return added later. Three reasons, kept apart (AP40): `phase 2 complete` /
  `phase 2 did not complete` / `hold failsafe`.
  **Measured, black-hole rig, one build: frame loop live at 258 ms, boot surface correctly still
  up, phase 2 landing behind it; control (`?boot=inline`) 3252 ms; 0 FRAME SKIP.**
  Gate: `make e2e-worker T=the_two_phase_boot_arms` — **three claims, each falsified**: the loop
  arms without the document (neuter `boot_inline_requested()` → `ROW 8 RED`, log tail stopping at
  *"phase 1 complete"*); the page is **not** handed over yet (neuter: call `surface_down()` from
  `frame_loop_live()` → reds on `FLICKER`, which is the shape that was rejected); and a phase 2
  that never reports back cannot hold the page (neuter: delete the failsafe `spawn_local` → reds
  with the surface still up). **The central assertion is INVERTED from the flagged version** — the
  old one demanded the surface be *down*, i.e. it encoded the flicker as the feature.
  **The prior handoff's review recipe is void and the reason is worth carrying:** it prescribed
  eyeballing both orders on localhost, where `/entity-deployment.json` answers in **0.4–5.8 ms**
  against a 16.7 ms frame. A *prescribed manual check is a test rig* — ask whether it can exhibit
  what it is written to exhibit before you prescribe it, and state the magnitude someone is being
  asked to perceive. The answer was not to build the rig; it was that the flicker did not have to
  exist.
  **`EntityApp::boot_document_read` is a SEPARATE associated fn and that is load-bearing, not
  tidiness** — it is awaited holding **no app borrow**. A `boot_phase2(&mut self)` that fetched
  its own document would hold `borrow_mut()` across the one await that actually stalls, which is
  §4a's blocker reproduced with a flag on it. **Hoisting it also removed a guard rather than
  adding one:** there used to be two `read_document()` call sites under exhaustive, mutually
  exclusive conditions, and the `!config_was_absent` guard that kept it to one fetch is the one
  whose absence took G1 from 3244 ms to 6195 ms. One call site holds that structurally.
  **The phase boundary is a DATA-DEPENDENCY boundary, not a rendering one — and a gate had to say
  so.** The first cut put the §4-B surface spawn in phase 1, on the obvious reasoning that the
  phase whose job is *paint something* should pick the surface.
  `rekeyed_domain_heals_on_next_boot_window_surface` red **on the default path** with *"stale
  cached outline shown: true"* — the re-key incident's own symptom. Phase 1's
  `peer_supersession::load` loads the records a **previous** boot wrote; `persist` writes *this*
  boot's A→B and is phase-2 work by construction, because it is discovered from the document. A
  window spawned before it hydrates a location naming the retired publisher, which is the contract
  the code states in `boot_load` itself. **The overlay survives the same ordering and a window does
  not** — the overlay has a re-point (`config_was_absent || adopted_identity`), a spawned window
  has no equivalent, and that asymmetry is what makes this look arm-specific when it is not. So the
  spawn, the `?site=` deep-link navigation and the remote-fixture seed all sit **behind the
  adoption**. Anything that resolves a durable reference through state phase 2 discovers belongs in
  phase 2, however much it looks like "what the user sees" — and the resulting chrome-then-window
  on a `surface=window` deployment is not a defect to engineer away, it is the flicker §4 priced,
  made visible.
  **`AGENTS.md` said the scope was "widen one handle" and that understates it.** `DispatchHandle`
  really is `Clone` with the async twins available — but **every phase-2 helper takes `&Peers` by
  reference** (`roster::read_roster_async`, `peer_supersession::load`/`persist`/`revalidate`,
  `origins::adopt_deployment_origin`, `mirror_to_all_local_peers`, `WindowView::hydrate_durable`)
  and `Peers` is owned **by value** in `EntityApp` — so holding one across an await holds the app
  borrow. The real scope is *widen the handle **and** convert each phase-2 reader off `&Peers`*.
  **Stated bound, not buried:** the deferred *local* tree work still holds the borrow. That is a
  stutter on a painted page, not a blank one — measured at **0 skipped frames**, and the gate
  **prints the `FRAME SKIP` count rather than asserting a threshold nobody has earned**, so a
  regression shows up as the number climbing. Gate:
  `make e2e-worker T=the_two_phase_boot_arms` — black-hole rig, with the **shipped order as an
  in-session control** (a second `connect_browser()` contends with its own first on a
  single-slot node). Background: `AUDIT-BOOT-PATH-2026-08-27.md` **§4c**.
- **A bad build has somewhere to fall back TO — row 10, C9+C10, 2026-09-02.** Two halves that are
  only testable together (a pin with nothing to pin to has no target; a `builds.json` nothing reads
  is a file).
  **C9, the publisher half:** `entity-browser builds <DIR>` / `make builds-manifest`
  (`src/build_slots.rs`, native-tested; `site-dist`'s third sub-make, so it reads the shell that
  will actually be served). Retains the shell at `/builds/<build_id>/index.html` and writes
  `/builds.json`. **`build_id` is the BUNDLE HASH, the commit is only a label** (§3.1) — two
  docs-only commits produce byte-identical wasm, which §4A.0 measured on the live fleet, so they
  are one slot. **The limit that will bite someone: a change confined to UNHASHED assets
  (`sw.js`, the worker pair) does not move the id and is not a distinct slot.** Three things the
  naive version gets wrong: a republished id **keeps its index** (a docs-only release must not push
  the ordering forward); `next_index()` is **max+1, not `len()`**, so a prune cannot hand a later
  build an index a client already stored as its anti-rollback floor; and a **malformed**
  `builds.json` is a hard stop, never an implicit fresh start — *"there is none"* and *"there is one
  and I cannot read it"* decide different things. `--prune` is opt-in and **never removes an asset a
  surviving shell names**: a retained build whose bundle was pruned is a slot that 404s at exactly
  the moment someone falls back to it.
  **C10, the client half:** `window.__ENTITY_BUILD_SLOT__`, the **first `<script>` in
  `index.html`, plain JS, before the module script** — the same tier as `__ENTITY_RECOVERY__` and
  for the same reason, since anything needing the app to boot in order to escape a build that will
  not boot is not a recovery mechanism. `build-stamp.sh` now stamps **`entity-build-id`** (the
  bundle hash) beside `entity-build` (the commit) so the gate can tell whether it *is* the pinned
  build before the body is parsed; it is derived from the same reference `build_id.rs` and `sw.js`
  parse, so it is a convenience for the pre-WASM tier, **not a second source of truth**.
  **Retained shells boot from any path only because trunk emits ROOT-ABSOLUTE asset references and
  the worker registers at root scope** — if that ever becomes relative, every retained shell 404s
  its own bundle and the mechanism silently stops working, which is why the gate asserts the app
  **booted** there and not that the URL changed.
  **THE PIN IS A LEASE, NOT A DEED — three independent ways out**, because a rollback a user cannot
  fall out of is a brick with better manners (audit F2, already shipped once): a **TTL**;
  **self-clear** when `/` no longer serves what the pin rolled *away* from; and an **attempt
  counter** so a pin naming a missing shell heals instead of stranding someone. That third one is
  the difference between recovery and a new brick and is easy to miss: **a 404 means none of our
  code runs at the target, so it cannot self-heal there** — it heals on the next visit to `/`,
  which is what a stuck person does. Armed before the redirect, disarmed **only** on arrival.
  **Recovery is exempt from redirection**, since it is the surface that can clear a pin.
  Gate: `make e2e-worker T=a_pinned_build_is_honoured` — falsified three ways (drop the attempt
  counter → *"a NEW brick built by the mechanism meant to remove one"*; drop the recovery exemption
  → *"eat its own escape hatch"*; drop the self-clear → *"a lease that outlives the reason it was
  taken is a deed"*).
  **Stated bound: nothing in the product SETS a durable pin yet.** That is C11's crash-loop counter
  and §3.5's recovery Boot section. Today the mechanism is reachable by a hand-typed
  `?build=<id>` — real for an operator or support, inert for everyone else. **Do not describe row
  10 as closed.**
  **RE-MEASURED 2026-09-08 — one domain has slots now, five do not, and the split is the fact to
  carry.** `entitychurch.org` serves `/builds.json` **200** listing **two** builds
  (`7a1118c2630bd81d` @ `d9cc645` index 1, and a retained `70e3e3d69e547fb4` @ `ef3a7e1` index 0,
  `min_rollback_index: 0`), and **both slots' shells AND their bundles resolve 200** — so row 10 is
  real on a live domain for the first time. It also serves `/sw-selfdestruct.js` **200**, so C17 is
  pre-staged there. **The other five — `ecdeos.org`, `entitychurchfoundation.org`,
  `entitycoreprotocol.org`, `entitychurchregistry.org`, `billslab.com` — are 404 on BOTH**, because
  all five still serve the pre-C9 bundle `6a41dc151b1b09ba`. **The first publish to any of them
  creates its first slot and still has nothing to roll back to.** Re-measure with a `curl` per
  domain; do not quote this paragraph forward.
  **And the fleet is genuinely non-uniform now, unlike the 09-02 false alarm** — two distinct
  bundles, not one bundle with two labels. `6a41dc151b1b09ba` carries **three** commit labels
  (`1ad7ca4`, `56c0921`, `10a5398`), which is still ONE build and ONE rollback slot per §3.1.
  **Every one of those five is `(unstamped)` on `entity-core-ref`**, so the kernel half of their
  pair is unrecoverable from the artifact — the stamp landed 2026-09-05 and they predate it.
  Headers are correctable on all six (`fleet-probe` exit 0).
  **Original bound, measured 2026-09-04, kept because the reasoning is the transferable half: a
  mechanism that shipped is not a mechanism a deployment HAS.** `/builds.json` and
  `/builds/<live-build-id>/index.html` both 404 on all **six** live domains — the retained-build
  machinery landed 2026-09-02, after the last deploy, so no publish had ever written one. The
  consequence is the one that matters on a cutover day: **the FIRST publish to use it still has
  nothing to roll back to**, and cannot retroactively retain the shell it is replacing (that shell
  was built from another branch and is not in hand). It creates the first slot, which pays off from
  the *next* publish onward. What covers the gap in the meantime is the correctable-header property
  — `fleet-probe` exit 0 — so *republish* is the recovery, and that is measured rather than assumed.
  **The general shape: a capability present in the build reads as a capability the deployment has.**
  Same family as a gate satisfied by its fallback and as *recording a gap is what makes it look
  handled* — ask what the ORIGIN serves, not what the artifact contains. One `curl` per domain.
  **A default that is also a MEANINGFUL value collapses *absent* into *unreadable* — and pick the
  arm by which mistake you can afford (AP40, applied to a parse).** `BuildsManifest::from_json`
  hard-stopped on a malformed `builds`/`index` and then read the anti-rollback floor with
  `unwrap_or(0)`. A `min_rollback_index` that was **present but unreadable** — a string, a float, a
  negative, a representation widened by a newer publisher — became **0**, which is not a neutral
  default: it is the one value that disarms `--min-rollback-index`'s refuse-to-lower guard *and*
  gets written straight back out by the next ordinary publish, erasing the floor with **no flag and
  no warning**. The field carrying the safety property got the only silent default in the parser.
  Absent (or explicit `null`) is still a real zero — a first publish must not be a hard stop — and
  present-and-unreadable now fails the whole document, like everything else in it. Gate:
  `a_floor_that_is_present_but_unreadable_is_malformed_not_zero`, seen red first.
- **ROLLBACK IS PARTIAL, AND `sw.js` CACHED THE ROLLED-BACK SHELL AS THE CANONICAL ONE.** Two
  findings at the C9/C10 × service-worker seam, which no gate covered because the two features were
  built four days apart and the rollback gate asserted nothing about the worker.
  **(1) Fixed:** `networkFirst` cached **every** navigation under the canonical `/`. That rule was
  written when every navigation *was* `/` (only the query varied); **C9 added a second navigable
  document**, so honouring a pin overwrote the offline shell with the rolled-back build — which
  then **outlives all three of the pin's ways out**, because TTL, self-clear and attempt counter
  each only run on a load of `/`, and offline `/` is served from that entry. `currentBuildId` reads
  it too. `isCanonicalShell` now gates the put on `pathname === '/' || '/index.html'`; a retained
  shell caches under its own URL, which also makes a pinned build work offline for the first time.
  Measured before the fix, not reasoned: cached `/` reported the retained id while the origin
  served the live one. Gate: the `1b` block in `a_pinned_build_is_honoured…`, **seen red first**.
  **(2) NOT fixed, and it bounds what row 10 can promise: a rolled-back shell runs against the
  CURRENT worker and the CURRENT `sw.js`.** Those are unhashed, the origin serves exactly one of
  each, and there is no `/builds/<id>/entity-worker_bg.wasm`. `AGENTS.md` already records C9's
  limit in one direction (*a change confined to unhashed assets does not move the build id*); this
  is **the converse — a build id that moves does not carry the unhashed assets with it**. So
  **if the bug you are rolling back from is in the worker or in `sw.js`, rolling back the shell
  does not escape it.** Do not describe row 10 as "a bad build has somewhere to fall back to"
  without this qualifier.
  **CORRECTED 2026-09-05, and the correction is the interesting half — it UNDERSTATED the defect
  and misnamed the mechanism.** This entry used to close by blaming `dropSupersededBuilds` for
  storing the current worker "under the rolled-back id". Traced through the code (meta DevOps
  raised it; their mechanism was wrong too, in the same direction): **`dropSupersededBuilds` is
  not implicated, no eviction is needed, and no fetch is needed.** `currentBuildId` reads
  `cache.match('/')` — and `isCanonicalShell` guarantees a retained shell is *never* written to
  the `/` key, while the `/index.html` fallback fetches the live build from the origin. **So both
  reads resolve to the CURRENT build, always.** A page on retained shell A resolves to B and is
  handed B's worker **on a cache hit**, correctly keyed under B. The cache stays coherent; the
  *page* does not. Net pairing: **A's shell + A's main bundle** (content-hashed, so `cacheFirst`
  gets it right) **+ B's worker**.
  **So `sw.js`'s "unreachable by construction" was true of the mechanism it described and false as
  the guarantee readers drew from it.** The invariant `buildScopedAsset` holds is *"the worker
  matches `/`"*, never *"the worker matches the running shell"* — one sentence before C9, two
  after. C9 split the collapsed value and **only the write side was fixed** (`isCanonicalShell`);
  the read side was left inferring the running page from `/`. Same family as AP40, and the same
  bug the `isCanonicalShell` comment itself narrates, one layer over.
  **What bounds it, measured:** `WorkerProxy::new` checks `protocol_version` against the main
  bundle's compiled-in `PROTOCOL_VERSION` and returns `VersionMismatch`, and `main.rs` falls back
  to Direct/IDB — so a **wire-incompatible** skew fails closed and the app still boots. Two builds
  sharing a `PROTOCOL_VERSION` (the common case; it moves only on a wire change) are **not**
  detected and simply run. Reachable only where a worker is spawned: `?worker=1`, or a profile
  holding a persisted `Backend*` peer (`respawn_persisted_backend_peer_into`, which spawns one on
  **both** arms) — a default fresh profile spawns none.
  **Why no gate saw it:** `a_pinned_build_is_honoured_and_every_way_out_of_the_pin_works` mentions
  the worker **zero times**, and it *cannot* exhibit this — `stage_two_build_spa` synthesises the
  retained shell by rewriting only the `entity-build-id` meta, so the retained shell still names
  the **live** bundle and `BUNDLE_HASH` reads the same id from both. The rig makes shell and worker
  agree by construction. **A two-build rig that shares one bundle is not two builds** — any gate for
  this needs a genuinely distinct second bundle.
  **The fix is to make the requester name its own build**, not to retain a worker per build:
  `app.rs::worker_loader_url` already composes the loader URL and the main thread knows its id
  (the `entity-build-id` meta), so passing it there and reading it off
  `clients.get(event.clientId).url` closes it. Do **not** key on the worker's own bytes — that
  reintroduces the disagreement the scheme exists to prevent.
- **TORI IS NOT A SMALLER BROWSER — audit it against its OWN substrate, 2026-09-02.** The heal-path
  arc is browser-shaped and most of it genuinely does not apply: `frontendDist` is embedded in the
  executable, so the WebView has no service worker cache of a remote origin, no CDN, and no build
  slots; there is **no updater plugin**, so the "auto-reload users into a broken build" risk (S-5)
  is browser-only. Verified sound on its own terms: `app_server` looks assets up by **exact key**
  in the embedded map (traversal structurally impossible), `redirect_target` percent-encodes CR/LF
  (no header injection), `sanitize_name` is an allowlist `[a-z0-9-_]` capped at 32 (no traversal via
  peer label), and C15's shared rule is real — `#[path = "../../src/cache_policy_rule.rs"]`.
  **Three things that DO cross over, and one of them inverts:**
  **(1)** Tori is a **publisher** — `app_server` serves the SPA to a phone over `http://<lan-ip>`,
  which is **not a secure context**, so that browser gets **no service worker at all** and
  therefore no offline shell: it cannot open the app when the desktop sleeps, even though its tree
  is in IndexedDB, which works fine on an insecure origin. `readiness.rs` enumerated *"the two
  losses that are uniform"* (OPFS, `getUserMedia`) and there are **three**.
  **(2)** **Desktop has rollback EXPOSURE without rollback MACHINERY**, which is not the same as no
  exposure — and §4C's premise (*"nothing could ever run an older build against newer data"*) is
  true of the browser and **false of Tori**. `data_root()` is `$HOME/.entity` with no version in
  the path, and reinstalling the previous installer is ordinary user behaviour. So §4C.2's lossy
  round-trip is reachable today with no floor, no version check (`ROSTER_SCHEMA_VERSION` is written
  and discarded), and the **longest** exposure, since no updater carries anyone forward. **The
  inversion worth carrying: the browser's risk is high-probability/short-duration, Tori's is
  low-probability/indefinite** — so "out of scope for rollback" must not be read as "no rollback
  risk."
  **(3)** Coverage is thin where it is most privileged: `src-tauri/src/lib.rs` is **1516 lines with
  2 tests** and holds the entire IPC command surface.
- **NO GATE COMPILED `tests/e2e_worker.rs` — `make lint` had the same hole `make test` is already
  documented as having.** The file is `#![cfg(feature = "e2e")]`, so `make test` compiles it to
  nothing (this file said so); plain `cargo clippy` builds neither test targets nor that feature,
  so **~25k lines were type-checked by nothing but an 11-minute Selenium run on a box with a grid.**
  `make lint` now runs `cargo clippy --features e2e --tests` as its second step — **compiling needs
  no grid**, and it costs ~48 s for the whole lint. Falsified both ways with a deliberate type
  error: plain `cargo clippy` exits **0** and never sees it; `make lint` exits **2** with
  `error[E0308]`. The transferable half: *this file already recorded the `make test` half of the
  hole, and recording it is what made it look handled* — a known gap in one gate is not a reason to
  assume the neighbouring gate covers it. Check what each gate actually compiles, not what its name
  suggests.
- **AP47 has a live instance whenever a `poll_json` result is consumed by `map_err` alone.**
  `poll_json` returns `Ok(last_value)` on TIMEOUT and its own doc says *"the caller still asserts"*
  — so `.map_err(|_| "the worker never finished wasm init")?` named a condition it could not
  detect. Found in `the_worker_bundle_is_fetched_once_per_build_not_once_per_load`, where it was
  the **vacuity guard for loads 2..N**: the downstream `fetches == 0` branch only catches a worker
  that never spawned *at all*, while a worker that spawns on load 1 and silently fails afterwards
  also yields `fetches == 1` — the passing value — so the gate could report *"fetched once across
  3 loads"* for exactly the wrong reason. Now asserts on the returned value; falsified by pointing
  the log filter at a string that never appears, which **timed out at 31 s and still returned
  `Ok`** — the clearest possible demonstration that the old `map_err` was unreachable. **Audit
  rule: grep `poll_json` call sites for `map_err` and check each one asserts on the value.** One
  site had it; the other 72 assert correctly.
- **UN-NAME BEFORE YOU REMOVE — and a set that is only PRINTED is not a guard.** `--prune`
  deleted retired shells and left their entries in `builds.json`, so the manifest advertised slots
  that 404. Latent today (nothing reads `builds.json` at runtime yet) and aimed squarely at
  **C14's slot list**: a *"Boot this version"* button that 404s at the moment someone is falling
  back, where **none of our code runs**, so C10's attempt counter can only heal it on a later visit
  to `/`. The publish ordering (§3.2: entry → `/` → `builds.json`) run backwards is the rule —
  un-name, then remove; **a shell nothing names is unreachable, a name with no shell is the
  failure.** Dropping the OLDEST entries cannot move the counter, because `retained` keeps the
  newest and `next_index` is max+1 — verified end-to-end (prune 4→2, fifth publish took index
  **4**, not a reused 2), and falsified by neutering the drop. **The second half is the
  transferable one:** `assets_named_by` was computed, printed, and used by nothing, while the
  module doc called it *"the whole safety argument"* for prune — a guard on an operation that does
  not exist reads as wired to the next author. It now says so in the line it prints. Two limits
  stated rather than overclaimed: prune removes **shells only**, and a shell is ~100 KB against a
  bundle measured in MB, so it reclaims the small half while the hashed bundles accumulate.
- **A tool that answers *"what is deployed"* must use the identity the PUBLISHER uses — C15's class,
  one layer out.** `make fleet-probe` judged fleet uniformity on the `entity-build` **commit label**
  while `BuildsManifest::record` keys slots by the **bundle hash**, so the probe and the publisher
  held two expressions of *what a build is*. Measured on the first live run (2026-09-02): the two
  domains carry commits `1ad7ca4` and `56c0921` on the **byte-identical bundle `6a41dc151b1b09ba`**,
  and the probe reported *"NOT uniform — 2 distinct builds"* for what §3.1 rules is **one build with
  two labels, and one rollback slot**. The direction of the error is the costly one going into a
  release: it invents a rollback target that does not exist. Identity is the bundle hash; the label
  is printed beside it and is never the verdict. The id is derived from the bundle filenames
  `HASHED_REF` already matched, **not** by a second scan of the document — the required extension is
  what keeps a prose mention from being parsed as the build, which is the trap `parse_bundle_hash`
  hit on 2026-09-02.
- **A BUILD ID DOES NOT IDENTIFY A BUILD OF THIS REPO — the PAIR does, and the missing half is
  the one that moves (2026-09-05).** We link `entity-core-rust` by **path dependency** across
  twenty paths under `bindings/`, `core/` and `extensions/`, and there is **no cross-repo
  lockfile**. So the bundle hash is a function of *our commit* **and** *whatever is checked out in
  the sibling*. CI pins the second half (`CORE_RUST_REF`); **a local build pins nothing** — and
  the site publish is built locally, which makes this worse in exactly the place it matters most.
  `RELEASE-READINESS.md` §2 step 4 already warns that an unpinned ref lets "the same tag build
  twice into two different binaries"; that paragraph is written about CI and the local case had no
  entry anywhere.
  **Measured, and the way it surfaced is the transferable half.** A build id was handed to DevOps;
  a later **comment-only** edit to `assets/sw.js` moved it — which our own C9 rule says is
  impossible, since a change confined to unhashed assets cannot move the bundle hash. **Rebuilding
  twice at a fixed commit returned the same new id**, which is what separated *"the build is
  non-deterministic"* from *"an input you were not tracking changed"*: the build is reproducible
  against a fixed pair. Diffing the two bundles' **string tables** named the culprit in one step —
  an error token from `core/peer`, i.e. not our tree — and `git -C ../entity-core-rust reflog`
  showed two commits from another seat 20 minutes earlier. **When a hash moves and your diff cannot
  explain it, diff the ARTIFACTS, not the source.**
  **The intersection check is the one to run and it already existed:**
  `git -C ../entity-core-rust diff --name-only $OLD..$NEW` against the `path =` entries in
  `Cargo.toml`. Non-empty here (`core/tree/src/lib.rs`, `extensions/content/src/handler.rs`), so it
  was a real change in the shipped bundle, not an inert one.
  **Enforcement point: `tools/build-stamp.sh` stamps `entity-core-ref`** beside `entity-build` and
  `entity-build-id`, so a deployed shell states which kernel it was linked against and the question
  is answerable **from the artifact** rather than from someone's memory of what was on disk. Stamped
  as `unknown` when it cannot be read, deliberately — *"we could not tell"* and *"nobody recorded
  it"* are different facts (AP40). **`fleet-probe` reports the PAIR as of 2026-09-08** — the
  inventory line reads `pair=(commit, core-ref)`, a non-uniform fleet prints the pair beside each
  divergent build so *"same commit, two bundles"* names its own cause in one line, and a `-dirty`
  half on either side gets its own NOTE (*these bytes are not reproducible from any commit*). All
  three read through **one** `meta_content(name, html)` helper rather than a third hand-written
  regex — C15's rule applied to the probe, since `build-stamp.sh` now writes three tags. **Reported,
  never the verdict:** identity stays the bundle hash (§3.1), because two kernel commits can
  legitimately produce one bundle when the difference did not reach us, and `unknown` stays apart
  from unstamped for the AP40 reason above. Falsified against two fixture origins carrying one
  commit label, two bundles and one dirty half.
  **CONTROLLED AT BUILD TIME AS OF 2026-09-08 — `tools/build-pair.sh`, and `make site-dist` refuses
  before it builds anything.** CI pinned `CORE_RUST_REF`; a **local** build pinned nothing, and the
  site publish is built locally, so the pair was recorded and observable and not controlled. Now:
  `make build-pair` prints and checks it; `site-dist` runs `--check` first and prints the pair in
  its closing summary; **`CORE_RUST_REF=<ref>` is a real pin** — same variable name as
  `release.yml`, so the local path and the workflow express one intent one way — and it **verifies,
  never checks out**, because a sibling repo's git is read-only from here.
  **`build-stamp.sh` no longer computes the pair, it reads it** — the two expressions of one rule
  were C15's defect verbatim, so there is one file and both callers use it. It also resolves the
  sibling from **its own location** rather than from `cwd`, which is the same answer from the repo
  root and a defined one from anywhere else.
  **The hatch is `ALLOW_DIRTY=1`, and what it does NOT waive is the point.** A dirty tree is
  waivable; a **`CORE_RUST_REF` mismatch is not**, and the way out of that one is to unset the
  variable — which is an explicit statement that you are building against whatever is on disk, i.e.
  the thing you asked to be protected from. The first cut let `ALLOW_DIRTY` swallow both, caught
  while falsifying the guard: **a hatch named for one failure must not absorb another** (AP36, in a
  guard written the same hour). Four arms falsified — dirty/no-hatch reds, dirty+hatch passes,
  mismatch+hatch still reds, match+hatch passes — plus `site-dist` seen refusing on the real target.
  **Stated bound: this makes the pair MEANINGFUL, not reproducible-by-instruction.** Refusing a
  dirty tree guarantees both halves are *at some commit*, so a reader can check the pair out. It is
  not a lockfile and does not choose a ref for you.
  **Quote build ids as `(our commit, entity-core-rust commit)` in anything a deployer reads.**
  **AN UNEXPLAINED RED IN A SUBSYSTEM YOU DID NOT TOUCH IS A `git -C ../entity-core-rust log`
  BEFORE IT IS A BISECT — 2026-09-06, and the entry above did not prevent it.** That entry is
  written about a moving **build id**; this arrived as **18 failing tests**, which reads as your
  own regression, so the first instinct was to bisect a change that was nowhere near them. The
  tell is the *shape*: `make test` red at **1377/18** with every failure in one subsystem
  (tree writes — shell persistence, chat round-trip, memory-transport delivery) and all of it far
  from the diff. Cost: `dev` + kernel `e17d711` fails `make test` **and** four e2e gates
  (`a_shell_window_returns_to_its_working_directory_*`, `each_window_returns_to_its_own_slot_*`
  — **both arms of both**, which is itself the signal that the fault is *below* the arm split
  rather than in it). Confirm in two commands: stash and re-run (identical 18 ⇒ not yours), then
  the intersection check — non-empty here (`bindings/sdk`, `core/peer`, `core/tree`). Cause:
  0.8.2.11's **`put` admission ladder** — a peer now validates the carried `content_hash` and
  MUST NOT author one, and nothing lands through the app-tier writer (measured: `get -> None`,
  `0 entries` under `app/`).
  **AND THEN THE SECOND HALF, WHICH INVERTS THE FIRST: the defect was OURS** (the canonical-ECF
  entry above), fixed at `950054b`, `1517 / 0 / 19` and `67 / 0`. The routing doc is **withdrawn
  in place** with the retraction at the top rather than deleted, because the misdiagnosis is the
  lesson. **`git log` on the sibling tells you what CHANGED, never who is WRONG** — the
  intersection check did its job and then the write-up slid from *"the sibling moved"* to *"the
  sibling is at fault"* on no evidence from our own code, which is the 09-04 review's §7 exactly:
  *choosing the symbol after the claim produces a review that looks checked and is not.* **A
  correct instrument pointing at a real change is the most persuasive way to be wrong.** Use it
  to bound the search, then go and read your own code before you write the word "routed".
  **And the release is not implicated** — a deploy builds a pinned pair, so a sibling that moved
  after the cut cannot reach it; say so explicitly, because a red `make test` on `dev` reads as
  *the release is broken* to everyone who did not run the check.
- **ENTITY `data` IS CANONICAL ECF — `entity_ecf::to_ecf`, NEVER `ciborium::into_writer`.
  `tools/ecf-lint.sh` (in `make lint`, baseline-ratcheted) is the enforcement point.**
  `ciborium` faithfully preserves **your** map key order; `to_ecf` canonicalizes (**length, then
  lexical**) *"and the encoder gets no say"*. So an entity built with `into_writer` carries a
  `content_hash` over bytes the peer will never reproduce, and the moment it crosses L1 the put
  answers **`400 hash_mismatch`** — **silently, because `dispatch_write` is fire-and-forget.**
  Measured on `ShellState`: we wrote `wd, history, draft`; canonical is `wd, draft, history`.
  **The rule existed at ONE call site for a month and generalised nowhere — AP44, and this is its
  most expensive instance.** `registry_publish.rs`'s `http_poll_profile_entity` already said
  *"`to_ecf`, NOT `ciborium::into_writer` — and this is now load-bearing"*, written about that
  site's circumstance (a profile fetched by hash, where `verify_and_decode` re-encodes before
  hashing). Five other encoders never got it. When the kernel landed 0.8.2.11's §6.3 **`put`
  admission ladder** on 2026-09-06 — a peer now *validates* the carried hash and MUST NOT author
  one — the latent defect became fatal on **every L1 write**: `make test` 1377/18, four e2e gates
  red **on both arms**, every failure a tree-write path. Fixed at all five (chat ×3, shell ×2);
  green at **1517 / 0 / 19 across 19 binaries**.
  **How it was found is the transferable half, and the first two theories were both wrong.** The
  intersection check said `bindings/sdk` + `core/peer` + `core/tree` had moved, so it read as
  *their* regression and was routed as one. It is ours. **Trace, do not theorise (A1):** an L0
  `tree.put` of the real payload **succeeded**, and so did an L1 `dispatch_write` of a *synthetic*
  one (`a0`, an empty map — canonical by construction, which is exactly why it passed and why a
  simpler probe would have cleared the code). Only the real `ShellState` failed, and only through
  L1. `put_and_wait` — the awaiting twin of the fire-and-forget writer — is what printed the
  status. **When a write "does not land", get the Result: `dispatch_write` swallows it into a
  `tracing::warn!` that no native test has a subscriber for.**
  **The baseline carries a COUNT, not names** — a swap passes, same stated limit as `net-lint`.
  The 3 legitimate uses are not entity data (a fixture building a deliberately hostile body, a
  test helper feeding a decoder). **And note what this does NOT fix:** the L1 put wire sends `data`
  as a decoded `Value` for the peer to re-encode, so **any** non-canonical entity is lossy across
  it — which bears directly on `mirror_byte_fidelity`'s republication property, since a foreign
  entity we mirror is exactly the case where the bytes are not ours to canonicalize. Republishing
  through L0 preserves them; through L1 it does not. Routed, not solved here.
- **THE UPDATE PROMPT FOLLOWS THE BUNDLE, NOT `sw.js` — C7, shipped 2026-09-06
  (`src/build_update.rs`).** The old banner fired from the service worker registration's
  `updatefound` chain, i.e. **when `/sw.js`'s bytes change** — a `copy-file` asset touched three
  times all year against 72 `src/` commits. Measured on two consecutive production deploys of one
  domain: the deploy carrying the entire re-key fix left `sw.js` byte-identical and **notified
  nobody**, and an `sw.js`-only change would notify **everybody** about an application that did not
  move. The operator called the prompt unpredictable for weeks; it was deterministic and keyed to
  the wrong artifact.
  **The framing is the design, and it is not the obvious one.** The question is **not** *"what does
  the origin serve"* — it is *"would reloading this page get you different application code"*,
  because Reload is the only thing the button does. So the comparison is against **the freshest
  shell this browser can obtain**, and the check goes **through** `sw.js` rather than fighting it:
  online, `networkFirst` fetches `/` with `cache: 'reload'`; offline it serves the cached `/`,
  which is exactly what a reload would deliver — so a match is *correctly* quiet and a newer cached
  shell *correctly* prompts. Both arms right, no special-casing.
  **Identity is the bundle hash (§3.1), never the commit** — two docs-only commits would prompt for
  a no-op, which is the defect `fleet-probe` shipped. **It adds no fourth expression of "read a
  build id out of a shell"** (`build_id.rs`, `sw.js`'s regex, `build-stamp.sh` are the three): it
  calls the existing pure `parse_bundle_hash`. C15's rule, applied on the way in.
  **`PinnedDeliberately` is the arm a tidy version gets wrong** — a retained shell must not be
  nagged to leave the build it was rolled back to, or the banner argues with C10's pin. Detected by
  `is_canonical_shell(pathname)`, the same predicate `sw.js` uses, **not** by enumerating the
  boot-slot script's nine `action` strings, which would be a contract between two files with no
  compiler in between (the `boot_diagnostics.rs` shape one row over).
  **All five outcomes log, including `Current`** — a check whose only evidence of having run is a
  banner that did not appear cannot be told from one that never ran. That is also why the gate's
  **first** assertion is the anti-vacuity one: *"no banner"* is what an unarmed feature produces
  too. `showUpdateBanner` is **deleted** from `index.html` (it keeps a `console.info` for a worker
  update — real, and not actionable) and the prompt is translated in all 30 locales, which leaving
  it in the shell made impossible. `AUTO_RELOAD_ON_UPDATE` stays `false` (S-5).
  Gate: `make e2e-worker T=a_new_bundle_prompts`, falsified both ways. **And its own first run red
  for a defect in the gate** — the matcher was spelled `outcome="current"` while `tracing_wasm`
  renders `outcome = "current"`, so it would have blamed the feature. *A log assertion is a
  coupling to a formatter; normalise before matching, and dump the log tail on failure or the two
  candidate causes are indistinguishable.*
- **The live fleet is CORRECTABLE, measured 2026-09-02 (first real `fleet-probe` run, exit 0).**
  Both domains serve every mutable URL (`/`, `/index.html`, `/sw.js`, `/entity-deployment.json`) at
  `max-age=1, must-revalidate`, and both bundles `immutable`. So brick-matrix **#7/#8/#9 are not
  live risks**, a hotfix lands on the next refresh, and C17's kill switch is available *because*
  `/sw.js` is correctable. **And `dev` is not missing deployed work:** the deployed commits sit on
  `archive/dev-0.9.0`, reachable from neither `dev` nor `master`, which is a history rewrite at the
  release boundary (merge-base 2026-06-30), **not** lost work — zero code files under
  `src/`/`tools/`/`assets/` are deleted in `dev` relative to the deployed tree, and four spot-checked
  live fixes are present by symbol. Stated limit: that is a spot check plus a file-level census, not
  a line-by-line tree equality proof.
- **The cache-immutability rule has ONE expression and four call sites gated against it — C15,
  closed 2026-09-02.** `src/cache_policy_rule.rs` is the rule; `src/cache_policy.rs` wraps it for
  the app crate and `src-tauri/src/app_server.rs` takes it by `#[path]` module, so the two Rust
  servers compile **the same source**. Python (`tools/cors-serve.py`) and prose
  (`PUBLISHING-QUICKSTART` §6.2) cannot, so all four are pinned to **`tools/cache-policy-vectors.txt`**:
  the Rust tests read it (`make test`, `make test-tauri`), `tools/cache-policy-lint.sh` runs the
  Python rule over it, and `tools/cache-policy-doc-check.py` holds the document to the same two
  match expressions — both in `make lint`. **Add the failing path to the vector file first; it
  lands in every gate in the same commit.**
  **Why this was worth a module.** `REVIEW-2026-08-25` §2.1 found the rule written four times, no
  two the same, while `GOTCHAS.md` asserted they *"cannot disagree"* — a guarantee nobody enforced,
  false when written. Each expression passed its own tests, which is exactly why the drift was
  invisible. A mis-cached mutable file is brick-matrix cell **#9: no remedy at all**.
  **The rule itself changed in BOTH directions, and the shard test is the whole safety argument.**
  Immutable iff the path tail is `content/{aa}/{bb}/{hash}` **where `aa`/`bb` are the hash's own
  first four hex characters** — self-verifying, so a directory cannot satisfy it by accident.
  `contains("content/")` was the dangerous one (Hugo, Zola and Lektor all name their source tree
  `content/`, so an ingested site pins mutable HTML for a year); `starts_with("/content/")` was
  safe but silently stripped every **prefixed** deployment of immutable caching (`dist-federation`
  emits 92 such blobs). Measured across every published tree on this box: **7653 files under a
  `content/` segment, 7653 matching, zero exceptions.** The query string is stripped — it was not,
  in the one file operators are told to copy — and hex is lowercase-only, because widening the
  immutable set is the unsafe direction.
  **Falsified across the tree:** one line changed in `cache_policy_rule.rs` reds the app crate and
  `make test-tauri` together; the doc gate reds on a retired spelling returning to the rules table
  and on the rule moving without the document.
  **Two honest limits.** §6.2 is prose, so the doc gate proves it has not *drifted*, never that
  the CDN recipe is right. And the FORBIDDEN check is scoped to **table rows**, because a
  document-wide grep flagged the document's own note explaining which spelling was retired — a doc
  that may not name the rule it replaced cannot warn anyone off it.
- **`dist/` HAS NO `entity-deployment.json` — so `make serve` / `make build-serve` is a deployment
  that declares nothing, and any behaviour driven by that document is UNREACHABLE there.** Written
  after a human review of the two-phase boot came back *"no difference between the two"*: correct,
  and worth nothing, because the origin 404s the document, `deployment` is `None`, and both orders
  were sequencing the same empty set. **AP34/AP35 one layer out from the engine matrix** — green
  because the configuration that exhibits the behaviour was not in the population, not green by
  inheritance. `make site-dist` is the target that emits a deployment document; a plain `dist/`
  never has one — so **`make site-dist && make serve DIST=dist-site`** is how you serve a tree that
  actually declares something. **`serve` only started honouring `DIST=` on 2026-09-02**: it passed
  the literal `dist` while `site-dist` printed that exact command as its own closing advice, so the
  one invocation for reviewing the uploadable tree silently served the SPA-only one (AP37 — and the
  two trees differ in precisely the behaviour you would be comparing them for).
  To review anything document-driven by hand you need **all three**: the file
  (`printf '{"surface":"site"}' > dist/entity-deployment.json`, written **after** the build since
  `wasm-release` rewrites `dist/`), **a profile with no durable config** (private window — a
  persisted config always wins, so a warm profile takes the `Unchanged`/`LocalHome` arms and
  nothing moves), and both loads on that same build. **For the two-phase flicker specifically
  there is a FOURTH condition and nothing in this repo supplies it — a slow-but-answering
  origin.** The three above only make the deferred work *non-empty*; what makes it *visible* is
  latency. Asking someone to eyeball those three on localhost is asking them to see a sub-frame
  difference, which is the same error one layer further in: a recipe that cannot exhibit what it
  is written to exhibit. **And do not let *"we looked and saw nothing"* be recorded as *"the
  behaviour is acceptable"*** — they are different claims, and neither is available from a rig
  that cannot produce the phenomenon.
- **A falsifier that does NOT red can be a fact about the CODE, not a hole in the gate — and then
  the test is what changes.** Written for the two-phase gate: moving `spawn_local` above
  `boot_progress::armed()` left it green, and the reason is that the swap is a **no-op** —
  `spawn_local` queues, so the block runs after `start()` returns whichever line comes first, and
  no log a test can read distinguishes them. The assertion claiming to catch it was **removed
  rather than weakened**, and replaced with the comparison that *is* falsifiable (*armed before
  complete* — the inversion the feature performs). This is AP47's shape one layer out: **an
  assertion describing a condition it has no way to detect**, and it is invisible until the
  neuter is actually run instead of reasoned about. The rule the handoffs already carry —
  *a neuter that passes has two possible causes and you owe both* — has a **third**: the neuter
  landed, the gate is sound, and **the thing you neutered does not do what you thought it did**.
- **`wait_for_boot` MEANS PHASE 1, and since the two-phase boot became the default (2026-09-02) that
  is no longer "booted" — `wait_for_phase2` is the one to use when your assertion's subject is
  decided by the deployment document.** `wait_for_boot` polls for *"Frame loop started"*, which
  arms after phase 1's local reads (258–287 ms measured); the origins adoption, the supersession
  persist/revalidate, the startup surface and the `boot_diagnostics` routing mirror all run in a
  **spawned** `boot_phase2` behind a bounded network fetch. So `wait_for_boot(); client.goto(…)`
  **cuts phase 2 off mid-flight** — whatever it was going to write is simply never written, and the
  assertion downstream fails naming the *product* instead of the race. `wait_for_phase2` polls for
  `surface_down`'s line and **returns the reason** (`phase 2 complete` / `phase 2 failed` / `hold
  failsafe`) rather than a bool, because those are three different facts (AP40 — the same fix the
  boot code itself got). **Do not widen `wait_for_boot`**: ~100 call sites only need the app alive
  and painting, and the two-phase gate exists to observe the window between the two.
  **It is a CLASS, not a gate — seven live sites, and the census is `tests/e2e_phase2_barrier_census.rs`.**
  Every assertion in the e2e suite keying on a phase-2 log line (`deployment-config: applied`,
  `peer-supersession: recorded`, `the domain now publishes under a DIFFERENT identity`) was
  guarded by `wait_for_boot` and nothing else. Five were saved by an incidental `poll_rendered`
  standing in as a barrier; **two had none at all**, and the second of those is the shape that
  matters: `!first.iter().any(|l| l.contains("deployment-config: applied"))` — a **negative**
  assertion, which a lost race makes pass **silently and forever**, where a positive one merely
  reds and blames the product. *Ask which direction your race fails in.* The census runs in
  **`make test`** (it reads `e2e_worker.rs` as text, so it needs no grid and no `--features e2e`),
  pins the marker table **by count**, carries an anti-vacuity floor, and **ships its own two-way
  falsifier** — the `window_hydration_census` lesson, applied on the way in rather than after.
  Falsified against the real file: delete one guard → red, naming the file, line, marker, the
  barrier it found instead, and the fix.
  **Measured instance, and note it takes TWO halves — the second is the one people will miss.**
  `the_recovery_console_names_a_stranded_profile`, red 1 unfiltered run in 3 and **0 in 12
  filtered**. (a) `wipe_all_storage` deletes the peer databases *while the first visit's phase 2 is
  in flight*, so phase 2 resolves its config against a store that just vanished, lands on the build
  default — whose `home_site.peer_id` is **empty**, the documented "own peer" sentinel — and writes
  that into the mirror. (b) The next boot's phase 2, which would have overwritten it, is cut off by
  the navigation to the BIOS. **The poison and the thing that would have cleaned it up are separate
  bugs, and fixing only (b) leaves a gate that is green because it re-wrote a value it should never
  have had.** Both closed; falsified by neutering `write_routing_mirror` to `""`, which reds with
  `Raw mirror: {"home_peer":"",…}` — the flake's exact state.
  **And the gate that caught it could not have caught it, which is the transferable half.** Its
  healthy-case assertion was `healthy.contains(&peer_a)` against the whole card — but the card
  renders *"this domain publishes as {peer}"* from **the console's own fetch**, so it passed with an
  entirely empty mirror and only failed one step later, naming the wrong thing. Step 3's reconcile
  check had it twice over: with no `believed` the console takes its *"recorded no publisher to
  compare"* arm, which contains no `"STRANDED"`, and the domain line still names the new peer — so
  **a successful reconcile was reportable for a profile that had recorded nothing at all.**
  *When a surface renders two sources side by side, assert the LINE, never the card* — and when a
  gate reds at step N, check whether step N-1 was capable of failing.
- **`make e2e-grid` before you run the suite. "Flaky e2e" on this box was TWO INFRASTRUCTURE
  DEFECTS (here) plus ONE GATE DEFECT (the entry above) — all three fixed 2026-09-03. Do not write
  the word "flaky" again without re-reading both.** Unfiltered runs before the infrastructure fixes:
  41/24, 63/2, 64/1, reds moving between unrelated subsystems and each passing filtered.
  Immediately after them: 65/0, **64/1**, 65/0 — which is what made the residual a *findable single
  gate* instead of noise; fixing that one surfaced a **second** gate of the same class, which is
  what turned a gate fix into the sweep and the census. After all of it: **65/0, 65/0, 65/0**,
  three consecutive unfiltered runs on a fresh grid each (nine unfiltered runs this session in
  total; the full ladder is in `HANDOFF-2026-09-03-d`).
  **The lesson in the sequencing: fix the infrastructure first, because a real defect cannot be
  seen through it** — and then keep going, because the first real defect you find under it is not
  necessarily the only one. The 24-red run diagnosed the day before was a third separate cause
  again (fixture isolation); reading them all as one "flakiness" is what kept every one of them
  open.
  **(1) The grid had podman's default `/dev/shm` (512 MB) and had been up for six days.** Firefox
  renders through shared memory; under that ceiling a content process dies mid-test and the
  assertion that happens to be running fails. That is *exactly* the signature — failures that move
  and pass alone — and it had been read as a product-side race for weeks. Selenium's own image
  docs call `--shm-size=2g` a requirement, not a tuning knob. `make e2e-grid` always **replaces**
  the node, so a run starts on a cold browser with the right shm; `GRID_PORT=4455` moves all three
  ports (HTTP + both ZeroMQ bus ports — moving only HTTP dies with `ZMQException: Address already
  in use`). `make e2e-grid-down` when you are finished.
  **(2) Our containers took part in an SELinux relabel war over the SHARED PARENT.** Our bind
  mount is `<shared-parent>` (sibling path-deps resolve through it) and a sibling repo's
  container mounts the same parent with a private relabel, stamping its MCS categories across the
  whole tree mid-run; a container whose categories do not match then gets **EPERM on every file**,
  which surfaces as *"something else is holding :8092 and answering"* — because the server could
  not **read** `dist/` — and whose *first* casualty is usually a boot gate reporting *"0 log lines
  captured"*, which reads like the app. The old mitigation was `:z` on our own mounts, i.e.
  relabelling the shared tree back on **every** `make` invocation: joining the war, whose loser is
  whichever seat ran least recently, and paying a recursive relabel per container start.
  `PODMAN_LABEL_OPT := --security-opt label=disable` plus **no `:z` anywhere** means we neither
  stamp the other seat's tree nor depend on ours. **Falsified both ways** with
  `chcon -R -l s0:c111,c222 ./tools`: without the flag `ls: cannot access 'tools/ui-lint.sh':
  Permission denied`; with it, readable. Security delta ≈ nil — rootless containers already
  running as the invoking user over an explicit bind mount of the tree they build.
  **So do not `chcon` anything, do not run filtered to dodge a sibling, and do not report an
  unfiltered run as "blocked by the other seat"** — that advice is retired, and the two entries it
  lived in are replaced by this one. **Never kill the other seat's container** still stands.
  **What is unchanged and still bites:** the node is `maxSessions=1` and `setup()` reaps *every*
  session on whatever grid it is pointed at, so **one run at a time**. `make e2e-worker` now
  preflights through `tools/e2e/wait-grid.py --preflight`, which **reports** an occupied slot and
  proceeds (the reaper exists to rescue a leaked session; refusing would block the runs it is for)
  — silence there is what used to become a 240 s stall watchdog naming the wrong phase.
  **The grid's `ready` flag means "a slot is free", not "the hub is up"** — measured: it reads
  `false` while a run holds the slot. That makes it right for `e2e-grid` (fresh container) and
  wrong as a hard preflight, which is why the two use it differently.
- **A FAILING e2e test leaks its WebDriver session, and the next run then hangs wearing the
  costume of a product wedge.** An assertion panics *before* `client.close()`, so the session is
  never quit; the node is `maxSessions=1` and Selenium's `sessionTimeout` is **300 s**, so the
  next run blocks on the slot and dies at the suite's own 240 s stall watchdog — which prints
  *"Something below the assertions wedged — a hung renderer, a browser that stopped painting"*
  and names the wrong layer entirely.
  `setup()` calls `reap_stale_sessions()` for exactly this, **and it was reaping the wrong grid
  until 2026-09-01**: its URL was hardcoded to `:4444` while `connect()` honoured
  `E2E_WEBDRIVER_URL`, so a `WEBDRIVER=` run got the worst of both — the leak on the grid in use
  was never cleared, *and* live sessions on `:4444` were deleted by a run that was not using it.
  Fixed; both call sites read `webdriver_url()`. **Grep `4444` before adding a third** — the
  preflight had the identical hardcoding and was fixed in the same session, one hour apart,
  which is how a second instance gets missed.
  Verified rather than asserted: with a session deliberately left holding the only slot
  (`ready:false`), a filtered run now reaps it and completes in ~3 s instead of queueing.
  And **prefer one session per test** — a gate that opens a second `connect_browser()` contends
  with its own first one on a single-slot node (measured: 60 s, then a 240 s watchdog).
- **`make e2e-worker` fixtures shell out to `cargo test` AT RUNTIME — do not edit `src/` while the
  suite is running.** An unfiltered run came back with 11 failures all reading *"fixture
  `content_site::publish::tests::emit_… ` failed: could not compile"*, which reads like a product
  break and was a concurrent edit. The suite compiles `dist/` and the test binary up front, so it
  *looks* safe after that; the publish fixtures do not. Docs and `tools/` are fine, `src/` and
  `Cargo.toml` are not.
- **Run `make e2e-worker` for any peer-routing / arm-dispatch / peer-display change** —
  worker peer routes register *asynchronously*, so a fresh peer can be invisible while
  compile and unit tests stay green. The default browser arm is Worker-or-IDB, never
  Direct; a Direct-only test proves nothing about the shipped surface.
- **`make e2e-webrtc-file-crossengine` is the cross-engine gate, and it exists because its
  absence hid a shipped defect — AP34/AP35.** Every other WebRTC gate here is Firefox↔Firefox,
  which is the **one pair that cannot exhibit a data-channel message-size defect**: it
  negotiates `sctp.maxMessageSize` = 1073741823 where Firefox↔Chrome negotiates **262144**
  (both measured). A real Android(Chrome)→desktop(Firefox) transfer stalled silently while
  `make e2e-webrtc-file` stayed green — not green by inheritance, green because the failing
  configuration was **not in the test population**. `ENGINE_A`/`ENGINE_B` pick each side's
  image; the spike prints the engine the grid actually started, because a mixed run that came
  up same-engine would prove nothing. Run it for any change to the data-channel pump, the
  transfer batch sizes, or `file_offer`. **Cover the MIXED pair — two Chromes agree with each
  other exactly as two Firefoxes do.** WebKitGTK (the Tauri engine) is a third engine and is
  still uncovered.
- **Run the two WebRTC gates as a pair.** `e2e-webrtc-chat` is handed its connectivity and
  tests the transport; `e2e-webrtc-meet` makes the browsers earn it through the Shell and
  tests the product. When both are red, the pair tells you which layer moved.
- **Narrow while iterating, run unfiltered before landing.** `T=<test-name-substring>`
  selects among the tests; `UNTIL=<phase>` stops the phase monolith; `SKIP_BUILD=1` reuses
  `dist/` (dev only). There is no `FROM=` — the monolith is one stateful chain.

The harness rules that are easy to break — the stall watchdog, `ASYNC_ROUND_TRIP_BUDGET`,
`poll_json` instead of a fixed sleep, `PHASE_ORDER`, the session reaper, the display gate —
are in [the gotchas reference](docs/architecture/guides/GOTCHAS.md#testing--the-gates).

### Build-time knobs

- **Knowledge Base is opt-in** — default builds embed 0 docs. `KB_DOCS_ROOT=..`
  (workspace parent) or `=docs` (this crate); optional `KB_DOCS_MAX_AGE_DAYS` /
  `KB_DOCS_MAX_BYTES`. The build prints the embedded count.
- **`ENTITY_STARTUP_SURFACE`** (`build.rs` → `session_config::boot_default`) bakes the
  cold-boot **surface axis** only: `chrome` (default) / `site` / `window`, with
  `ENTITY_STARTUP_WINDOW_TYPE` naming the type when `surface=window`. A typo fails the
  build; it only seeds the absent-config case (a persisted config always wins on a warm
  boot). The build prints `startup surface: X`. The granular posture (`site_mode`,
  `peer_creation_enabled`, locked kiosk) is a per-domain `/entity-deployment.json`
  concern, **not** a build knob. This **replaced the opaque `ENTITY_PROFILE` presets**
  (`full`/`tutorial`/`strict-site`) — that variable is gone, and setting it does nothing.
- **Never set `KB_DOCS_ROOT` or `ENTITY_STARTUP_SURFACE` for `make e2e-worker`** — the
  e2e expects the default `chrome` cold-boot path and zero embedded docs.

## Code style & conventions

- **DOM-only:** to add/modify a window, implement `render_dom()` — there is no
  canvas path.
- **DOM events go through `DomCtx` helpers** (`on_window_event`, `on_select_change`,
  `on_action`, `listen`). **Never `Closure::forget()`** — it leaks permanently;
  closures live in `DomCtx.closures` and are freed on each rebuild.
- **Theme via tokens:** use `src/dom/theme.rs` constants (`BTN_PRIMARY`, `INPUT`,
  …), never inline style strings; reference colors as `var(--token, #literal)`,
  never raw hex (tokens in `src/theme_tokens.rs`). Authoritative:
  `REFERENCE-THEMING.md`.
- **UI atoms via `dom/components.rs`** (`button`/`button_action`, `text_input`
  (draft-tracked — typing survives rebuilds), `select`, `field`, plus
  card/table/chips/states) — never raw `create_element("button"|"input"|…)` in a
  view. Gate: `tools/ui-lint.sh` in `make lint` (baseline-ratcheted; migrations
  lower `tools/ui-lint-baseline.txt` in the same commit). Any new/changed window
  surface closes against **S1–S8** (`REFERENCE-UI-DESIGN.md` §2, Doctrine F5.5).
- **State lives in the entity tree, not Rust struct fields** — window structs
  hold only `window_id` + `peer_id`; the tree is the single source of truth.
  Don't build parallel data structures.
- **Make the access boundary visible:** `ctx.store()` (L0 sync) or `ctx.get()/.put()`
  (L1 dispatched); every `store()` is a visible security opt-out. Don't call
  `peer.tree().get()`, `peer.location_index()`, `peer.shared()` at runtime.
- **Change detection is subscription-driven** (`WindowWatch` wraps
  `ctx.store().subscribe`) — don't poll or hash.
- **Tree paths are fully qualified** `/{peer_id}/...` (leading slash) — never
  strip `peer_id`; the qualified path *is* the data model.
- **Reuse before abstraction:** parameterize/filter an existing view before
  extracting a shared component.

## Project structure (key `src/`)

- `peers.rs` — app-layer `Peers` multi-SDK router (`Direct(PeerManager) |
  Worker(WorkerPeerStore)`); per-peer ops route via `sdk_for(peer_id)`.
- `window_registry.rs` — single source for the window roster (`standard_window_types`).
- `app_paths.rs` — path conventions (`window_state_path`, `settings_path`);
  namespace `app/entity-browser/...`.
- `persistence.rs` — app-side persistence I/O (`load_persisted`, primary SDK).
- `storage_durability.rs` — ephemeral-fallback "not saved" banner logic.
- `writer_handle.rs` — `WriterHandle`, the arm-branching abstraction for app-tier writers.
- `dispatch_handle.rs` — `DispatchHandle`, its awaited twin for multi-step spawned flows.
- `reach_keeper.rs` — standing intent to be reachable to a met peer; the §6.5 mutual-attempt half.
- `file_offer.rs` — the browser as the **serving** side of a file transfer
  (`system/content` + an offer manifest); shell verbs `offer`/`offers`/`pull`.
- `boot_diagnostics.rs` — the routing mirror (`entity_routing_mirror` in localStorage): what
  publisher this profile is pointed at, when, and by which build. **A contract with the L1
  recovery console in `index.html`, checked by no compiler — grep both together.** It exists
  because the authoritative value is a CBOR entity in the tree and the BIOS cannot decode one
  by design. Rewritten every boot; nothing branches on it; no secrets.
- `window_watch.rs`, `dom/theme.rs`, `theme_tokens.rs`, `session_config.rs`.
- `src-tauri/` — Tauri desktop backend. `tools/e2e/` + `tests/e2e_worker.rs` — Worker E2E.
- `docs/architecture/{specs,guides,reviews}/`, `docs/plans/`, `docs/archive/`
  (read `docs/archive/INDEX.md` for swept history, not a session diary).

## Boundaries — do NOT modify

- **No SDK-tier code in this repo.** `EntitySDK`, `PeerContext`, `PeerManager`,
  `register_handler`, the `subscription` primitives live in the `entity-sdk`
  crate at `../entity-core-rust/bindings/sdk/`. This app is a *consumer* (the
  Godot binding consumes the same crate); SDK changes belong upstream so both
  benefit. Likewise `entity-shell` lives at `../entity-core-rust/bindings/shell/`.
- **Don't bake app conventions into the SDK** — path namespaces
  (`app/entity-browser/...`), on-disk persistence locations, renderer/runtime
  types stay here (`app_paths.rs`, `persistence.rs`). Don't add app-tier state
  fields to `PeerManager` or any SDK type.
- `bindings/*` (sdk / shell / worker-proxy) in entity-core-rust **is ours to fix**;
  only `core/*` is the kernel. The arm-split + subscription wiring are ours.
- **Upstream architecture docs win on overlap** — when an internal doc disagrees
  with `../entity-core-architecture/...` specs/guides, the architecture docs are
  authoritative. The `PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` is the
  authoritative arm model (prose arch docs on that topic are stale).
- **Current storage model is main-thread IDB-default** (Worker + OPFS is opt-in
  via `?worker=1`). Treat the old "Worker is the durable default / Direct is
  ephemeral" framing as superseded — don't re-derive from it.

## Before you report the state of this repo: `git worktree list`

**This repo has more than one worktree**, and a state claim scoped to one branch is not a
claim about the repo. That mistake has been made here in both directions — once as a
pessimistic claim (a capability filed as missing, read off a stale `Makefile` comment,
when it was already on `dev`) and once as an optimistic one that still misdirected ("go
look in the other worktree" for something already underfoot).

**Run `git worktree list`, `git branch -a` and `git merge-base --is-ancestor` before
answering "do we have X", and quote what you ran.** The same applies to sibling repos:
`git log --all -S<symbol>` across every checkout before any claim about another repo's
state — a feature living on an unmerged branch reads as a feature nobody built.

## Repo gotchas — read before you touch one of these areas

The load-bearing failure modes live in
**[`docs/architecture/guides/GOTCHAS.md`](docs/architecture/guides/GOTCHAS.md)**, grouped
by area. Every entry there was earned by a shipped bug. Open the section that matches what
you are about to change:

| Area | You are about to… |
|---|---|
| [Windows, DOM & rendering](docs/architecture/guides/GOTCHAS.md#windows-dom--rendering) | add or change a window, touch the frame loop, or call a fallible web API |
| [State, subscriptions & change detection](docs/architecture/guides/GOTCHAS.md#state-subscriptions--change-detection) | read the tree from a new surface, or add anything a render depends on |
| [Peers, SDK arms & the WASM substrate](docs/architecture/guides/GOTCHAS.md#peers-sdk-arms--the-wasm-substrate) | write across the Direct/Worker split, or thread a `peer_id` |
| [Connection liveness & reachability](docs/architecture/guides/GOTCHAS.md#connection-liveness--reachability) | show connection state, or record a fact about a remote peer |
| [Connectivity — WebRTC, rendezvous, NAT & relays](docs/architecture/guides/GOTCHAS.md#connectivity--webrtc-rendezvous-nat--relays) | touch signaling, ICE, transport profiles, pairing or the desktop node |
| [Publishing, signed roots & names](docs/architecture/guides/GOTCHAS.md#publishing-signed-roots--names) | change the publish path, the trust chain, or name resolution |
| [Content sites & documents](docs/architecture/guides/GOTCHAS.md#content-sites--documents) | change either content-site renderer, link resolution, or the document sandbox |
| [Apps & embedded programs](docs/architecture/guides/GOTCHAS.md#apps--embedded-programs) | change the app host, the iframe tiers, saves, or the programs surface |
| [File transfer & chat](docs/architecture/guides/GOTCHAS.md#file-transfer--chat) | change offers, the browse cache, or conversation binding |
| [Build, packaging & tree hygiene](docs/architecture/guides/GOTCHAS.md#build-packaging--tree-hygiene) | touch the Makefile, the release path, cache headers, `sw.js`, a boot-path fetch, or `.gitignore` |
| [Testing & the gates](docs/architecture/guides/GOTCHAS.md#testing--the-gates) | add a phase, a budget, or a gate of any kind |
| [The recovery console (L1 BIOS)](docs/architecture/guides/GOTCHAS.md#the-recovery-console-l1-bios) | touch `index.html`'s System Recovery screen, or anything it probes |
| [Localization, copy & the operator surface](docs/architecture/guides/GOTCHAS.md#localization-copy--the-operator-surface) | change a user-facing string, a refusal message, or a CLI output |

**The ratchet applies to that file.** When a feature or an audit teaches something, it
lands there (or here) in the same session, in the section it belongs to — and when you
land something that spends a stated concession, grep for the sentence it retires.

## Commit & PR

Default branch **`master`**; DCO sign-off required — see AGENTS-STANDARD.

**Push `dev` when you commit — do not ask.** Standing authorization, and it is operational, not
cosmetic: work is read from this tree at a named commit, so an unpushed commit is one nobody else
can fetch. Commit, then `git push origin dev`, in the same breath. The AGENTS-STANDARD floor still
binds — never force-push, never touch the Codeberg mirror, and a *tag* is still a deliberate
release act, never automatic.

**Do not tell the operator what happens after the current milestone.** Sequencing past the thing in
front of us is theirs to decide; ours is to report state, finish the work, and name what is ready.
A "this week / after Friday" split in a status report is the failure mode — report what is done,
what is blocked, and what is next-ready, and stop there.

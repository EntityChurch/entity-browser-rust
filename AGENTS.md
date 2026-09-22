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
  and **D24** (any durable copy of someone else's bytes is a cache and needs a currency
  trigger — ratified 2026-08-29 on a gate observed red, with both enforcement points);
  candidates at D17, D18, D22. The per-diff review questions (nine + 5b), anti-pattern
  catalog AP1–AP35.
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
  a Selenium container (headless Firefox) on `:4444`; recipe in `tools/e2e/README.md`.
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
make e2e-worker    # Worker-mode E2E, headless Firefox; Selenium :4444, serves :8092
make e2e-phases    # list the e2e test names + phase labels the filters accept
make noscript-check # the apex as a NO-JS agent sees it (crawler/text browser). Needs the
                   # same Selenium grid — do NOT run it beside e2e-worker
make e2e-webrtc-chat       # two browsers chat over §6.5 WebRTC — the MECHANISM
make e2e-webrtc-meet       # two browsers meet at a name then chat — the SHIPPED PATH
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
make registry BIND='--bind=NAME=PEER_ID@ORIGIN'   # publish a NAME REGISTRY + verify it
make federation    # the whole chain locally: N domains + a registry → dist-federation/
make native        # DEPRECATED — prints redirect, no native UI build
```

**Do not quote a test count from this file.** It goes stale in hours, the binary count
moves too, and "the main binary" and "the full suite" are different denominators that
session notes have quoted interchangeably. Re-measure, and say what you measured.

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
  unbounded and the baseline carries them by name. Read the GOTCHAS entry before "fixing" one.
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
  you may write down.** `deployment_config::read_document` is four-state (`Served |
  NoDocument | Unreadable | Unheard`) so the log can tell a domain that ships no config on
  purpose from one nobody could reach — but caching that 404 to skip the next probe would be
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
  `app/{app-id}/workspace/window-index` (type `app/entity-browser/window-index` — **app-internal
  per guide §4.1.1, not `app/state/…`**; one impl does not name a portable type for something it
  invented this week) lists the live windows as `(id, type_name, peer_id)` and buys claiming, an
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
  2026-08-31). `app/{app-id}/workspace/windows/{id}/state` is a cosigned cross-impl convention
  at **MUST** tier (`GUIDE-ENTITY-WORKBENCH-APP.md` §1/§3, three-impl consensus we cosigned;
  `entity-workbench-go/entitysdk/workspace_state.go:397` builds the identical path). The
  guide's §8 leaves the *lifecycle* to us in two named arms — *persist if the app offers
  session resumption; MAY be ephemeral otherwise* — and **we are in neither**: we persist
  per-window state and offer no resumption, which is what manufactures the collision. That
  decision is open and is the operator's:
  `docs/plans/DESIGN-WINDOW-STATE-LIFECYCLE-AND-SESSION-RESUMPTION.md`. Our per-content-type
  state names are the slot table's long-term shape, so AP42's guard is the convention working,
  not a local invention.
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

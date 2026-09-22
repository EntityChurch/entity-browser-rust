# entity-browser-rust (DOM/WASM) — AGENTS.md

Read **AGENTS-STANDARD.md** first. This file adds entity-browser-rust specifics.

## Overview

DOM-primary Rust/WASM application for entity-core: a window-manager / UI shell
over the entity tree, rendered as HTML DOM in the browser and in a **Tauri**
desktop WebView. Cargo package is `entity-browser-rust`; the on-disk dir was
renamed from `egui-entity-core-rust` to match. HTML DOM is the
**only** render path — the legacy native/egui renderer is gone, `EntityApp` is
wasm-only, and plain `cargo build` produces a deprecation stub.

### The public surface — what a version promises

**IN: what somebody else's work is built against.** The `entity-browser` CLI — its
verbs, flags, exit codes and refusals; the `make` verbs; and every format this repo
reads or writes **for another party**: the `--ingest` / `--ingest-feed` / `--gather`
source layouts, `/entity-deployment.json`, `/builds.json`, the published tree a
consumer fetches over HTTP (its paths and the entity type tags in it), the durable
tree shapes a returning profile is read back from (`app/entity-browser/…`,
`app/state/*`), and the `?`-query affordances an operator is told to type
(`?build=`, `?systemrecovery=`, `?worker=`).

**OUT: every Rust symbol in this crate.** Nothing depends on `entity-browser-rust`
as a library — it has no external consumer and no published crate — so the module
layout, the window models, the DOM structure, `tests/`, `tools/` and the e2e
harness move freely and never make a release breaking. The SDK is not ours either;
it lives upstream in `entity-core-rust` and versions on its line, not this one.

**So "breaking" here means:** an invocation, a deployment document, an authored
source directory, a published tree or a stored profile that worked at the last
version and does not now.

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

## ⛔ The ratchet's landing site — 113 earned lessons filed under the wrong heading

**Everything from here to the next `##` is 4,226 of this file's 4,611 lines — 92% — and none
of it is about building or testing.** The two bullets above this heading are; these are not.

It got here honestly. A lesson lands at the end of the section a bullet was nearest when it was
written, and `## Build & test` was nearest for the first one, so it was nearest for the next
hundred. **That is AP56 — *a fact filed under the exception is not filed under the rule* — which
this file already teaches, about itself, from inside the section it describes.**

**The entries are not the problem: they are earned, measured, and mostly load-bearing.** The
problem is that 113 of them share one heading that does not describe any of them, so the only way
to find one is to read 4,000 lines or already know it exists. `AGENTS-STANDARD` asks this file to
*name the trigger and not carry the content*, and by volume it does the opposite.

⚠ **Do not fix this by appending here.** A new lesson goes to the area file that matches it
(`GOTCHAS.md`'s table two sections down), and if none fits, say so in the handoff rather than
adding a 114th bullet to a heading that is already wrong.

**The migration is planned, not guessed:** `docs/plans/PLAN-2026-09-16-THE-CHARTER-IS-ONE-SECTION.md`
— which also records why it was not done in the session that measured it. A 113-entry scripted
move is precisely the operation this file's own *"verify a scripted edit by reading back the
ROW"* entry was written about, and the end of a long session is the worst moment to attempt one.

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
- ⭐⭐ **AN ENTITY YOU DID NOT AUTHOR TRAVELS AS BYTES; A STRUCT IS ONLY A VIEW OF IT — hold it in
  `Obtained<T>` (`src/obtained.rs`), which carries the reasoning, the measurement and the gates.**
  If someone else gives you an entity, **that** is what you store — not your re-encoding of the
  fields you happened to model. V7 §2.6's MUST-ignore governs *reading*; it licenses nothing about
  what you write back. **This is the second appearance of one root** (the first is the 09-10 asset
  regression's *"if your cache re-encodes rather than storing bytes, it can only cache what it
  fully understands"*, mitigated with a skip rather than fixed) — **a third, in a subsystem that is
  not `persist_to_cache`, makes it a discipline rather than this entry.**
  **The consequence is not the fidelity, and that is why it survived.** The content-site
  write-through and `foreign_cache::ensure_current` land at the **same key**
  (`ForeignArtifact::Manifest::store_path` *is* `paths::manifest_path`), and D24 decides *did
  anything change* by comparing the held `content_hash` against the origin's 58-byte hop-1 pointer
  — so a rewritten copy's hash can never equal the pointer it came from and every sweep answered
  `Fetched`, forever. **D24's mechanism defeated not by a missing trigger — the failure it was
  written about — but by a sibling writer at the same path putting bytes the trigger cannot
  recognise.** ⇒ ***when two code paths write one tree key, they owe the same bytes.***
  ⚠ **The arm that decides it is a defect contains NO unknown field:** an ordinary one-page site
  omits `nav` (SITE §4 makes it optional; *title-only is conformant*) and our encoder added
  `nav: []` — 25 B in, 30 B out, address moved. **Do not file this as forward-compatibility
  hygiene;** it was reachable by every conformant publisher who differs from us by one optional
  key. Same shape as `gpin4-joint`'s *same absence, two encodings*, arriving where it cost
  something.
  **Why a type and not a rule: it already decayed as a rule.** `foreign_cache` (written *for* D24)
  carries the `Entity` and is right; `feed_mirror` carries it and says so in a comment; the
  content-site resolver is older than both and was wrong at three call sites in one function while
  **its own doc comment claimed the writes were byte-faithful** (AP44). Nobody was careless — the
  decoded struct is what a renderer needs, so by the time anything wants to *store* the value the
  bytes are three frames gone. `Obtained<T>` derefs to the view (every read site is untouched) and
  `entity()` is the only way out. **`None` means WE authored it** — a synthesized section index, a
  notice — a real answer kept apart from *"we had bytes and lost them"* (AP40).
  **Enforcement point: `tools/fidelity-lint.py` (in `make lint`, baseline NAMES not counts).** It
  flags every durable write whose value re-encodes a decoded view; a row says *we authored this* or
  *the write IS an edit*, and a new site fails the build until somebody answers. It **fails both
  ways** — a row with no live finding is also red — because a SWAP is the dangerous case here and is
  exactly what `net-lint`/`ecf-lint`'s counts cannot see (`vocab-lint`'s lesson, second use).
  ⚠⚠ **ITS FIRST CUT WAS FALSIFIED BY THE VERY DEFECT IT WAS WRITTEN FOR, AND THE SHAPE IS GENERAL:
  a matcher that reads the WRITE's arguments misses a re-encode that was bound to a variable.**
  `let manifest_entity = rp.manifest.to_entity(); … seed_write(…, manifest_entity.clone())` — the
  argument is a name, so a one-level matcher sees nothing, and reinstating the real bug left the
  gate **green**. The asset and page writes at the same site inline the call and *were* caught,
  which is what made the hole look like coverage. Two levels of taint now (decoded → re-encoded
  binding → write), and the wider matcher immediately surfaced a site the first had missed. ⇒ ***a
  gate you falsified only with the defect's tidiest spelling is a gate you have not falsified*** —
  reproduce the ORIGINAL line, not a cleaned-up version of it.
  ⚠ **And the revert trap, one step past the `git checkout` lesson: a NEW tool file is untracked, so
  `git checkout` cannot restore it** — it errors, the neuter stays in, and the next run reports on
  neutered code. Restore by inverse edit, and re-run to confirm the gate is back before believing
  any green.
  ⇒ **Standing check when you add a durable write of anything fetched: ask what a `from_entity`
  dropped on the way in, and whether anything downstream compares the address.**
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
  ⭐⭐ **THIRD INSTANCE, 2026-09-15, AND IT IS THE TRAP INSIDE A PRACTICE THIS FILE RECOMMENDS: A
  PIN ON A CONTESTED SHAPE HAS AN EXPIRY YOU DO NOT CONTROL.** The first two instances are a rule
  carving an exception out of a rule we satisfied. This one has no exception in it. FEED §2.4
  declared `? cursor: content-hash`, §4.4 required `{page, applied}`, we routed the contradiction
  and **deliberately emitted the declared field** on `G-PIN-4`'s rule — *whatever publishes first is
  the baseline* — with `the_declared_cursor_is_a_bare_hash_and_carries_no_page_number` pinning the
  shape **so that a ruling would move one file.** That is the right call and this file teaches it.
  **v0.3 ruled by DELETING the field and adding `FEED-R35` (MUST NOT)**, and from that moment the
  test asserted our non-conformance under a name that reads as a documented decision. Two days
  green, and it is the second time a landed rule in this convention has found us through a passing
  test. ⇒ ***when you pin a shape "so a ruling moves one file", the ruling you are not expecting is
  "the field is removed" — and that ruling turns your pin into a bless.*** The cheap instrument is
  the one that caught it: **a pin against a contested clause is the first thing to re-read when the
  document's version number moves**, because its whole premise is a sentence in somebody else's
  draft. Note the fix's shape too: the field is **absent** from the struct rather than deprecated or
  `Option`-and-never-set — a slot that exists is a slot somebody fills, and the test that replaced
  the pin is a floor rather than the only defence.
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
  ⭐ **THIRD INGREDIENT, 2026-09-11, and it defeats the assert this entry recommends: AN EARLIER EDIT
  IN THE SAME RUN CAN MOVE A MATCHING ROW INTO YOUR NEXT ANCHOR'S RANGE.** Moving four ruled asks
  Open → Closed, then inserting a new row "after the last `| **A-4` in Open", put it in **Closed** —
  because `A-41` had *just* been moved there and now sorted last. **The assertion passed**, correctly:
  the line really did start with `| **A-4`. **An assert that your anchor MATCHED is not an assert that
  it matched the RIGHT one**, and no predicate over the row's own text can tell them apart. What
  caught it was the script **printing the resolved target** (`INSERT after 259 -> | **A-41** …`) and
  that line being read. ⇒ **anchor on a row id that is unique in the whole file, never on a prefix;
  assert the target is in the SECTION you mean (`open_i < d < closed_i`); and print what you resolved
  to, because in a multi-step edit the tree you are anchoring against is one you already changed.**
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
- ⭐⭐ **A REPORTER THAT LIVES INSIDE THE THING THAT DIES IS NOT A REPORTER — the dead-instance
  detector, `index.html`, 2026-09-15, and it is the WASM tier of the argument G8 already makes
  about the recovery console.** Measured on a real session: the instance stopped resolving its own
  closures and the page threw `RuntimeError: index out of bounds` from a wasm-bindgen closure shim
  **388 times**, alternating between two shim signatures. Not a Rust panic —
  `console_error_panic_hook` was installed and printed nothing; on a closure-invoke shim that
  message is a `call_indirect` at a function-table index the table does not have. **After the first
  one, not one more tracing line was emitted** — not even the 5-minute `build_update` interval that
  had been ticking right up to it. That silence *is* the unresponsiveness.
  **THREE DETECTORS, ALL SILENT, AND NONE OF THEM WAS BROKEN.** `diagnostics.rs` installs `window`
  error/rejection capture whose module doc states its whole purpose is that "a browser-level failure
  is captured and visible *in-app*" — **zero** lines against those 388 errors, because the handler is
  itself a wasm closure. `watchdog.rs` does the hard part right (the watcher is off-thread in a
  Worker) and then reports back through a main-thread wasm `onmsg` closure, so the watcher kept
  ticking into a corpse. And it is installed `show_banner = false`, so even a correct detection drew
  nothing. ⇒ **when you build a failure reporter, name the failure it cannot report, and check that
  is not the one you built it for.** All three of ours had the same blind spot and it was the whole
  subject.
  **The detector is plain JS in the shell** — same tier as `__ENTITY_RECOVERY__` and
  `__ENTITY_BUILD_SLOT__` — and it **reports and offers, never acts**: no auto-reload, because a
  crash-loop reload is a brick generator and the WASM-load retry one block up already records what a
  post-start auto-reload cost (it wiped live sessions on a denied `clipboard.writeText`). **A banner
  rather than a modal, and that is F2's lease-not-deed instinct applied to a verdict** — if we are
  wrong about the app being dead, a full-screen overlay is us bricking a working session on our own
  false positive; hence the Dismiss beside the Reload and the Recovery link.
  **Four outcomes (AP40) and the two collapses both cost something.** `pre-start` is not a weaker
  `isolated`: before the module executes, an error belongs to the WASM-**load** auto-retry, which
  owns that failure — counting it here puts *"the program stopped"* over a boot that never started,
  a different fault with different advice. `isolated` is not a weaker `dead`: one freed closure is a
  defect, not evidence the instance is over.
  **The record names the FIRST trap and keeps its stack.** The 388th is an echo of a closure that
  was already gone; the first is the only one whose stack names it. Written to `localStorage`
  (`entity_last_trap`) under the routing mirror's rules — bounded, **nothing branches on it** — and
  **System Recovery renders it**, because a record only a devtools console prints is one the stuck
  person on a phone cannot read (the console's own founding argument, one screen over).
  **Gate: `make e2e-worker T=a_dead_wasm_instance`, six rows, six neuters, each landing on a
  distinct row** (no threshold · no trap classification · verdict with no banner · record follows
  the latest trap · no durable record · no recovery exemption). **Scope, stated in the gate's own
  doc comment so the name cannot be read wider than the measurement: it gates the DETECTOR, not a
  trap.** The events are synthetic and dispatched at the real `window`, so everything above the trap
  itself is the production path; producing a real trap needs a deliberately broken bundle and is a
  different rig. `?deadinstance=<n>` lowers the threshold — a test affordance, same shape and same
  reason as `?bootstall=`, and nothing in the product sets it.
  ⛔ **WHAT KILLED THAT INSTANCE IS STILL OPEN.** The obvious candidate — a failed §6.5 negotiation
  leaking a live `RTCPeerConnection` over freed handlers — is **covered**: `discard_session` runs on
  the failure path and calls `pc.close()`, and `main_thread_establish.rs` states the rule in its own
  header. Do not report it as the cause. Standing context from that session, none of it established
  as causal: a 32 s frame gap (`wake_probe` `gap_ms = 32005`) immediately before the first trap; a
  §6.5 channel that had opened and then died, followed by ~100 re-negotiations (the trailing numbers
  on those log lines are Firefox console repeat counts) all `role=answerer, sdp_exchange=INCOMPLETE,
  candidates posted=0/fed=0`; sustained `HIGH REBUILD RATE 11–14/sec`, 4438 rebuilds; and the alpine
  VM mounted with ~172 MB of bundles. **The build was `-dirty`, so those two shim hashes are not
  resolvable from any commit** — which is the build-pair entry's own subject arriving as a
  diagnostic cost rather than a release one.
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
- **A MEET IS A HUMAN-TIME ACTIVITY AND ITS WINDOW WAS A POLL COUNT — the pacing change,
  2026-09-16 (`rendezvous::poll_interval_ms`, `SEARCH_WINDOW`).** Reported by the operator as *"it
  just starts shooting up to 60, it stresses me out, and I don't know what the duration is"*. **Both
  halves were one defect:** the window was `MAX_POLLS = 60` at 30 frames apiece, so the only number
  the surface had to show was **the counter itself**, climbing toward a bound that meant nothing to
  the person watching it. ⇒ ***when a surface can only render a loop's internal counter, the bound is
  in the wrong unit*** — a wall-clock window has a duration to report and a frame-counted one does not.
  **120 s at ~29 round trips, against 30 s at 60** — four times the window for half the traffic, and
  the cost is *gated rather than claimed* (`the_longer_window_costs_fewer_round_trips_than_the_old_one`,
  falsified at `got 240`). Pacing out is what buys it, and the asymmetry is why it is nearly free:
  **whoever presses second finds the first one on their FIRST poll**, because the request is already
  in the bucket — so the interval bounds only *how long the person who pressed first waits to notice*.
  8 s, not the TTL. **Answering stays unpaced**: a queued answer is a peer already waiting on us.
  **THE INTERVAL MAY NEVER APPROACH THE NODE'S TTL, and the TTL is ADVERTISED.** An interval that
  straddles a deposit and its expiry loses a peer with **nothing erroring anywhere** — §2.2's silent
  never-meet, one mechanism over from the lobby constant. `Limits::ttl_seconds` (§4.5) rides back from
  `resolve_key` on the one path that already had to ask (lobby), the default comes from the
  extension's own `Limits::default()` rather than a `60` typed in here, and it **narrows only**: a
  node advertising an hour does not license a slow surface, because the human's wait bounds the top
  too. Falsified — drop the clamp and the gate reds with *"an interval of 8000ms at ttl 8s can outlive
  a bucket entry"*.
  **The display is COARSE so a renderer cannot put the number back.** `TimeLeft::{Minutes,
  UnderAMinute, NotSearching}` — the seconds never reach the view, the phrase moves at most once a
  minute (so it needs no repaint of its own; the poll marks the window far more often than the phrase
  can move), and `NotSearching` is kept apart from a zero, because a settled meet has a result to
  report and **no clock at all**.
  **Browser evidence is OPPORTUNISTIC and the run says so.** Phase 14.7's node is unreachable by
  construction, so catching the search mid-flight is a race; the `else` arm prints *"the mid-search
  line went UNMEASURED this run"* rather than leaving a green suite to imply coverage. ***An
  assertion that may not have executed must not be read as coverage, and the place to say so is the
  RUN, not a doc.***
- **"IS THERE AN ASSUMPTION OF ONE SIGNALING NODE?" — YES, AND THE PROTOCOL'S ANSWER IS SPECIFIED AND
  DEFERRED COHORT-WIDE. Do not build it here (measured + operator-scoped 2026-09-16).**
  `EXTENSION-REGISTRY` §3b defines a `service-advertisement` entity carrying a `signaling:
  [{endpoint, priority}]` **pool**, and §3b.2 makes intra-pool selection a **`[cross-peer seam —
  MUST]`**: both peers rendezvous-hash the §3b.3 weight (pinned to the byte) and independently select
  the *same* member. Priority-order or a load balancer **splits the pair and the punch never
  completes**.
  **All of it is implemented and none of it is called.** `entity_signaling::pool::select` exists with
  unit tests and a live-rendezvous test; **every caller in `entity-core-rust` is a test**, and
  `entity-core-py` has the same module. §3b's own preamble records why: *implementations built
  §3b.2's pool-selection rule and had no protocol way to learn a pool to select*. The registry
  extension's status line marks §3b **folded v1.5, implementation deferred**.
  **Our model is a conformant pool of size 1** — N rows in the connector registry, exactly one in
  force (`connectors::node_in_force`) — so this is not a conformance gap. What it costs is that
  *"both peers must meet at the same provider"* lands on **the user**: two people who each picked
  their own node never meet and nothing errors. Making the node visible (2026-09-16) is the mitigation
  that fits a manual pool.
  **Two reasons the meet form defaults to `tag` and not `lobby`, and the second one is the operator's
  and is about scale:** a lobby meet is a stranger, *and* the lobby key is a single constant, so under
  §3b.2 **every lobby meet in the world hashes to one shard**. A named tag distributes. Recorded
  because the code states only the first reason and the second is what will matter the day a pool
  exists.
  ⛔ **Scoped out by the operator 2026-09-15**, before this measurement: the live/rendezvous tier
  (`T5`) is off the release path — *"a visitor who wants a live peer connection brings their own
  rendezvous"* — and the hold's condition is **a release going out, not a date**. So the pool is
  deferred work behind a deferred tier. **Do not route this to arch as news**: §3b's preamble already
  says it, and reporting it would be *a landing reported as a discovery*.
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
  ⭐ **SECOND INSTANCE 2026-09-17, in a different instrument, so the rule generalizes: A PROBE THAT
  REFUSES MAY BE REFUSING YOUR RIG.** Arch asked both app seats for one falsifier they cannot run
  (*"present a grant carrying `/{A}/system/signature/*` to a conformant peer and confirm it is
  accepted"*, `ROUTING-2026-09-17-a` §6), with an open ruling resting on the answer. It came back
  **400**. The refusal was `peer_pattern` — a hand-typed `"2KSOMEREADER"`, which `configure`
  validates and rejects — and the resource under test was never reached. **Run with the subject
  alone, that is a packet saying "your ruling is wrong" on a measurement of my own typo.** What
  caught it: two **controls in the same loop** — patterns the shipped `share_feed_with` authors
  every day — failing *identically*. ⇒ ***when a probe refuses, put a value you already know is
  accepted through the same call before you believe the refusal is about your subject.*** The
  needle version of this rule is about a zero; this is about a non-2xx, and both are *the
  instrument answered and you read the wrong variable*. Leave the controls in the gate rather than
  deleting them once green — they are what makes a future red legible.
  **And the experiment found what it was not asked about, which is the usual return on running
  one:** `peer_pattern` is validated and the **resource** pattern is not, so a grant naming a
  mistyped author is stored without complaint and then matches nothing — no error at write, none at
  check. Defensible (a resource pattern is a pattern, and `/*/…` has to be legal) and worth knowing
  before any surface renders *"shared with N"* off a stored grant.
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
- ⭐⭐ **A RECORDED EMPTY ORIGIN IS SAME-ORIGIN, AND READING IT AS ABSENCE UN-NAMED A PUBLISHER
  PERMANENTLY — found in production 2026-09-17, by the operator, on the first real registry walk.**
  Resolve a name, press *Open*, and that publisher is gone from the whole profile: the Feed window
  says *"this deployment does not know where they are hosted"* and the Site Browser rail says **"No
  sites yet"**, on the same boot that logs `warm_peer_sites: cached 7 foreign site manifest(s)`.
  **The bytes were in the store; the name was gone.** `RegistryBrowserModel::open_in_site_browser`
  is the only product caller of `set_origin` and stores the binding's origin **verbatim**; a
  single-domain deployment binds same-origin (`--bind=<name>=<peer>@/`, `"origins": {"<peer>": ""}`),
  so `origins::decode_origin`'s `if origin.is_empty() { None }` made the write **invisible to its
  own reader**.
  **The permanence is the other half, and it is D25 working correctly.** That row is marked
  `SOURCE_USER`, so boot's adoption refused to repair it — `outcome = KeptUserOverride { theirs: "" }`
  in the log on every subsequent boot, measured across two reloads — and `unname_withdrawn_origins`
  skips `user` rows by design. **Nothing in the product could heal it.**
  ⇒ ***fixed at the READER, not the writer.*** Expanding `""` in the one caller is what boot does,
  and boot's comment states the rule (*"the registry treats an empty origin as unregistered, so we
  store the concrete URL"*) — so repairing that caller would have left the rule as something the
  next writer must know (AP44) and pinned a concrete host into a `user`-marked row that is never
  refreshed again (AP50's shape one field over, where `""` relocates itself if the domain moves).
  `""` is this codebase's spelling of same-origin in **four** places and every URL builder already
  trims it. Three facts stay three (AP40): no row · an unreadable `origin` key · a **recorded** `""`.
  ⚠ **THE MISSING POPULATION WAS NOT AN ENCODING OR ANOTHER IMPLEMENTATION — IT WAS THE ORDINARY
  DEPLOYMENT.** `the_open_writes_into_the_same_store_the_spawned_window_reads` passes with the defect
  fully present (**measured**), because its fixture names a concrete host; and the one e2e gate that
  resolves a registry name is **deliberately cross-origin** and carries a control whose whole job is
  to stop its rig degrading into same-host — right for what it measures, and it means one domain
  serving its own publisher was in no browser population anywhere. ⇒ **when a gate's fixture picks
  one arm of a value's domain, ask which deployment shape the other arm is.**
  **Enforcement, three points.** `every_origin_a_writer_can_store_is_one_the_reader_reads_back`
  (`origins.rs`) is the class: the round trip over **every** value a writer can hand us, through both
  accessors, with the count — *a write that succeeds and cannot be read is the silent shape*.
  `a_publisher_bound_same_origin_is_still_routable_after_the_open` is the journey's decision. And
  **`make e2e-worker T=a_name_resolved_from_a_registry` is the browser gate the surface never had** —
  `opens()`' own doc had said so since it was written (*"this repo has no gate that drives the
  Registry Browser"*), which is the cost of a gap being a sentence. Its rig
  (`emit_registry_walk_fixture`, in **`registry_publish.rs`** — `publish-doc-check` scans
  `publish.rs` for every `"--flag` literal and `--bind`/`--issued-at` are the registry's) publishes a
  registry, a publisher's sites and their feed into **one** origin. All three falsified; the browser
  one reds at row 5 with the production sentence verbatim.
  ⇒ **Standing check: when a surface stores a value somebody else authored, read the decoder for a
  filter on it** — and if the decoder drops a legitimate value, the write lands, the surface looks
  correct, and only the *next* reader is wrong.
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
  follows, not speculatively before.** ⇒ **DONE 2026-09-10 on exactly those terms —
  `src/publish_axes.rs` is the list, and the feed is what paid for it.** See the axis entry below.
  (`ForeignArtifact` staying a hand-edited closed enum is **correct** — that is D24 forcing the
  currency question per kind.)
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
  without authorship**. Run this gate for any change to the emit path, the store round-trip, or a
  mirror/republication surface.
  ⭐⭐ **THE SENTENCE THAT USED TO SIT HERE — *"anything mirroring foreign bytes must carry an
  inclusion proof (the author's root + the trie path, cheap because HAMT nodes dedup) or accept that
  authorship needs the author's origin"* — IS RETIRED, AND EACH OF ITS THREE CLAUSES WAS WRONG
  (measured 2026-09-14, `src/verification_ladder.rs`).**
  **(1) There is a third option and we ship it.** A **detached `system/signature` at the invariant
  pointer** gives authorship with no root, no trie and no origin — `feed_read::attribute` takes **no
  store, no index and no source**, deriving the author's key from the author's *peer id* and
  rebuilding the expected signer hash locally. Gated by
  `a_rung_1_entry_still_names_its_author_with_the_origin_deleted`, which `remove_dir_all`s the
  publisher's whole tree (and asserts it is gone) *before* verifying. ⇒ **an entity can be made to
  speak for itself, and ours already are on one axis**: `publish_feed` mints one per entry
  (`faf73cb`), so `n` entries publish `n+1` signatures against a site publish's **exactly 1**.
  **(2) "Cheap because HAMT nodes dedup" prices a mechanism that does not exist.** Named search for
  `inclusion proof` / `verify_inclusion` / `proof_path` / `merkle proof` / `trie proof` across
  `entity-core-{rust,go,py}`: **zero, all three** — and `SignedSession::resolve` takes a **source and
  a key**, so **nothing anywhere takes *(entity, root, path)*.** Rung 2 is **origin-shaped by
  construction**: carrying it means reconstructing something that looks like the author's origin.
  **(3) It is not merely heavier — it PERISHES, and in the RECIPIENT.** Identical carried bytes are
  `Declined("rollback")` by a reader who has seen the author's current tree and **accepted** by one
  who has not (both arms in `a_carried_snapshot_is_refused_once_the_author_publishes_again`). So a
  snapshot is neither valid nor invalid on its own terms — **a mirror handing one bundle to two
  readers gets two answers** — and the anti-rollback floor that makes a signed root worth having is
  exactly what refuses one. `Declined`, never `Verify`: neither carrier nor author has a defect.
  **The cost, so nobody re-derives it: a signature is 207 bytes flat and two files, whatever it
  signs** — +110% on a 189-byte post, +0.7% on a 30 KB page, +0.04% on our largest published figure.
  Constant, not proportional ⇒ **the axis is not which entities are important, it is which entities
  TRAVEL**, and bytes are not the argument against signing broadly.
  ⛔ **What this does NOT buy, and it is the first wrong inference to guard against: signing content
  does not sign a CLAIM about content.** `EXTENSION-SUBSTITUTE`'s admission gate
  (`verify_entry_signature_against`) demands a signature whose **`target` is the chain entry's hash**
  — an entity in *my* tree asserting where A's content lives — while an entry's invariant signature
  targets **the content hash**, which `find_signature_tree_resident` cannot match. Two separate asks.
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
  (AP44) rather than a third arm — **which is what shipped** (`src/publish_axes.rs`, 2026-09-10).
  **⚠ AND THAT PREFERENCE IS NOW A CONSTRAINT, MEASURED 2026-09-10: TWO PROJECTOR RUNS INTO ONE
  DIRECTORY DO NOT COMPOSE, SO A STANDALONE `feed OUT_DIR` VERB IS THE WRONG SHAPE.**
  `a_second_axis_signed_by_its_own_projector_un_names_the_first` — publish a site, then publish a feed
  with its own `RootProjector`, and the site reads back **`Err("Absent")`** while the feed resolves.
  `finish` builds the trie over **`self.bindings`**, the keys *that projector* recorded, which its own
  doc already said: *"the root commits to the bytes we projected, not to the tree we read from."* The
  second `finish` signs a root naming only its own axis. **The bytes are still on disk**, so it is an
  un-naming rather than a delete, and the consumer says *the publisher withdrew this site* about a site
  that is right there — AP54's wrong sentence from the opposite direction. The gate asserts the bytes
  survive precisely so the report is legible as *wrong* rather than merely bad.
  ⚠ **THE FIRST WRITE-UP OF THIS DREW THE WRONG CONCLUSION FROM IT, AND THE CORRECTION IS THE ENTRY.**
  It called `publish` *"stateless per run and destructive"* and said a feed needed *"a source for
  everything I have ever posted"*, which the operator rejected on sight — **and reading
  `resolve_publish_source` says they were right.** The pipeline is already
  **`ingest_path(disk) → tree → read_all_sites(tree) → project`**: publish **translates an input into
  the tree and then projects the tree**. So the clean is not destruction, it is what a **snapshot**
  means — *not* re-projecting is what leaves an output stale against the tree — and there is **no
  "source of everything" problem special to feeds**, because the source is the tree and the tree is the
  authority. `--ingest`'s `render/` dir is one workflow into it, not the model.
  **So the finding is narrower and better than the alarm it was first written as.** What the
  measurement rules out is *a second, independent writer putting bytes into a directory publish owns*.
  The other order (`a_feeds_bytes_live_inside_the_subtree_a_site_publish_cleans`, feed at
  `{base}/{peer}/app/feed/…` inside `clean.push(base.join(peer_id))`) is **not a defect at all under
  the right model** — a re-projection carries what is in the tree it read, and a feed written behind
  publish's back was never in it. It is kept as a **path fact** because it is the cheapest statement of
  *why* the second writer is the mistake.
  ⇒ **The shape is ONE projection over the tree, with a third axis read out of it** — a
  `read_all_feeds` beside `read_all_sites` / `read_all_app_sets`, recorded through the same
  `RootProjector`. **Built 2026-09-10; the reader is `read_owned_feed`, singular, because §4.2 pins
  ONE index path per peer.** **No new durability model, no read-back, and `C-2`'s second-device bound is not
  engaged**, because nothing recovers state from the out-dir. That is also exactly what the
  registration table above is: *what publish reads out of the tree* is the policy, and the hardcoded
  pair is the defect.
  **`resolve_publish_source` building a FRESH in-memory peer is a real gap and it is F3, pre-existing
  and not feed-shaped** — it is what *"publish cannot publish a peer's tree"* means, it applies to
  sites identically, and it blocks *"Tori posts"* rather than blocking feeds.
  ***Two transferable halves.*** (1) *Before building a verb that writes into an output directory
  something else owns, publish the other thing there first and see what survives* — two orders, two
  tests, twenty minutes. (2) ***A measurement is not its interpretation.*** The numbers here were right
  the first time and the sentence around them was wrong, because *"a projection re-projects"* was read
  as *"a publisher destroys"*. **When a measurement makes an existing, working pipeline look broken,
  the pipeline is the thing to go and read** — one function (`resolve_publish_source`, 35 lines)
  dissolved it.
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
- **`G-PIN-4` IS GREEN ON THE SUBSTRATE, AND THREE BLOCKERS WE WERE ABOUT TO FILE WERE ANSWERED BY
  ONE SENTENCE IN A SPEC NOBODY HAD OPENED (2026-09-09).**
  `src/content_site/crossimpl_reproducible_publish.rs` (in `make test`) runs
  `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §9's *reproducible-publish* case — one fixture, two
  publishers, one root — against `entity-workbench-go`'s **vendored** deterministic emission, and
  passes at root `00f567bf…`. **entity-core-rust vs entity-core-go, so §2's *"independent means built
  over different cores"* is satisfied**, and this is the app tier's first byte-equality evidence that
  is not cohort-consistent. Falsified: one perturbed body reds both halves, naming the key.
  **The move worth copying is that it needed nothing from anybody.** The other seat's vendored
  fixture **is a second implementation you already have** — it had been sitting in
  `tests/fixtures/crossimpl-go-site/` since August, read only as *"can our reader walk their
  bytes"*, when the same bytes answer *"do our publishers agree"* the other way round. **Before
  opening a coordination thread for cross-impl evidence, check whether the counterpart's artifact is
  already in your tree.**
  **The comparand is the TRIE ROOT, and our own tooling prints the wrong thing under the right
  words.** `system/peer/published-root` carries `published_at` — `core/peer/src/published_root.rs`
  `now_ms()`, no seam — so two `make site` runs of ONE fixture under ONE pinned `--identity-seed`
  gave heads `008615f3b44c09c7…` / `00a7b337ea9fd493…` with **`root_hash` identical**
  (`00bc252f2a685c4a…`) and 13 of 15 content blobs shared. `make site-dist` prints that head as
  *"signed root"*. **A G-PIN-4 rig comparing it reds 100% of the time, and across two impls it reds
  wearing a real divergence's clothes.** `EXTENSION-TREE` §3.2 determinism rule 3 is the property to
  reach for: *"No timestamp — a snapshot is pure structural data."*
  **Three blockers, one sentence, all wrong in the direction of MORE coordination.** §3.2: *"Two
  peers with identical content under different prefixes produce the same trie root hash"* — bindings
  are keyed **relative to the prefix**. So **placement is irrelevant** (theirs
  `content/sites/{id}/…`, ours `sites/{id}/…`; relative to the site root both spell `manifest` and
  `pages/{slug}`), **no shared keypair or peer id is needed**, and neither seat moves. And the
  `ENTITY-CBOR-ENCODING` §5.4 two-arm risk the last handoff said to settle *before building a rig* is
  **withdrawn**: §5.4 is about *receiving* foreign bytes, and a publisher authoring from source
  canonicalizes on either arm. Every one of the four was reasoned from our own code. *If nothing you
  opened contradicted you, you did not run a check* — and the corollary this adds: **a blocker that
  asks for more coordination is the one to re-read the spec about**, because it is the shape a wrong
  inference takes when the honest answer is *"nothing is needed."*
  **The vocabulary half is NOT established by that green** — their fixture's leaves are `test/note`.
  `tests/fixtures/gpin4-joint/` is our side of it: `site.json` (language-neutral **authored input**),
  `EXPECTED.json` (every site-relative key → content hash + the trie root, computed from our
  `app/site-manifest` / `app/site-page` encoders), a `README.md` carrying the protocol, and
  `gpin4_joint_fixture.rs` pinning it so our encoder cannot drift silently under the other seat.
  **Every row of that fixture is a case where two impls can each be internally consistent and still
  disagree** — a page with **no** frontmatter (absent ≠ empty map, `omitempty` on their struct and
  conditional on ours), two frontmatter keys (canonical map order), a nav section with no `target`, a
  nested `children`, a non-default `format`, two levels of nesting so the trie has interior nodes. A
  fixture without them goes green while the divergence is live.
  **`EXPECTED.json` is a wire artifact: regenerating it is a wire event, not a test fix.** The other
  seat compares against those bytes, so if they move without a deliberate `site.json` edit, our
  encoder moved and the question is which side is right.
  **`G-PIN-3` is a different shape and `app/site-root` is its only reachable target** — the
  published-root cannot carry *expected signature bytes* because it carries a clock; the pin
  (`{root, seq, site_id, ? passthrough_of}`) has none, and Ed25519 is deterministic. Not implemented
  here and not proposed unilaterally (arch's read is that we are not being asked); routed as
  `A-22` so the withdraw/keep decision is made with it in hand.
  Packets: `ROUTING-2026-09-09-g-workbench-go-…` (the fixture + the ask) and
  `ROUTING-2026-09-09-h-arch-…` (the §9 wording clause + what we withdrew). `K-3` on the
  `entity-core-rust` tracker is the publish-instant seam, which **blocks none of this**.
  **`F-5` — the nav depth/cycle vector — is built too (`static_export.rs`, mod `nav_depth_f5`), and
  building it found that §4.1's DoS property holds BY ACCIDENT, IN A DEPENDENCY.** Measured across
  twelve depths: an authored nav ≤126 decodes fully, **≥127 makes `SiteManifest::from_entity` take its
  `Err(_) => Self::default()` arm and discard the WHOLE manifest** — `site_id` and `title` with it,
  silently, indistinguishable from an empty one. Nothing overflows at any depth to 1,000,000, but what
  stops it is **the CBOR decoder's nesting limit**, not us — so the test **reds if that limit moves**
  rather than tracking the number, because that is a supply-chain event and not a constant to update.
  **Not repaired here: the fix is in a module that compiles into the frozen release bundle**, and the
  shape (partial decode vs a distinguishable *"refused"*) is arch's — `A-23`.
  **Two scope facts that reading §4.1 does not give you.** (1) **§4.1's walking contract lands entirely
  on the STATIC EXPORTER.** The live DOM renderer maps `manifest.nav` **one level** into flat links and
  never reads `children` (`output_from_resolved`; its own `GAP 3 (sub-nav)` note) — so nested nav is not
  rendered live at all, and *"the renderer"* is two things here. (2) **The cycle half is unreachable by
  construction** — `NavItem::children` is an owned `Vec<NavItem>` and CBOR carries no back-reference, so
  a visited-set would guard a state the type system forbids. Answered structurally, and put to arch as a
  question rather than assumed.
  **`MAX_NAV_DEPTH` exists because there were TWO walkers and only one bound.** `render_nav_items`
  carried an inline `32`; `subtree_holds_active` — same tree, same authored data, returns a `bool` so no
  markup assertion can see it — had **no bound at all**. C15's shape: the half nobody reads is the half
  that drifts. Both read one constant now, and **both neuters were run** (one reds on `got 41` vs
  `right: 33`, the other by name).
  **§9's *"pinned depth so 'stop cleanly' is not vacuously conformant"* is the clause to honour
  literally** — assert the bound from **both** sides, or a renderer that walked one level passes.
  **`F-1` CLASSIFIES TWO `ref` FORMS AND EVERY `ref` WE SHIP IS THE THIRD — read this before touching
  embeds, and note the vector is deliberately NOT built (`A-24`).** §3.2: a leading `/` is a `path`,
  *otherwise* a `content-hash` in multibase/hex — exhaustive by construction. Ours are all
  `assets/figures/x.png`, and `paths::asset_name_from_ref` **rejects** a leading `/` and **rejects** a
  bare hex, accepting only the `assets/`-prefixed site-relative form — which is what the authored
  content, the papers render tool and `markdown_to_embed` all produce. **The two spaces are disjoint.**
  **And our refusal is a SECURITY property, not an oversight** — subgraph confinement, one predicate
  covering `://`, `//`, a leading `/`, `data:` and `..`, gated by
  `asset_name_from_ref_accepts_site_local_and_rejects_external`. So `F-1`'s `path` arm asks for exactly
  the shape that check treats as *not site-local*, and "conforming" is a security review rather than a
  parser change. Likely reading (offered, not claimed): `F-1` was written for the sibling-`Embed` `ref`
  and ours is a third site-scoped reference the convention has not named — under-scoped, not divergent.
  **Do NOT write the vector to the rule and let it red.** A test asserting behaviour we deliberately
  refuse is a disagreement wearing a gate's clothes — AP45's inverse, where the *name* would make one
  arbitrary reading look settled.
  **And check what a vector needs to EXIST before pricing it.** §9's embed case is *directive ⇔
  **child-`Embed`***, i.e. directive ⇔ **entity**; we have **no `Embed` entity type** (`app/embed` over
  `src/` returns one doc comment). Our `Embed` is a Rust struct that never becomes an entity. The first
  draft of that packet called this half unblocked and next; it is neither, and **one grep** corrected
  it. Same for the passive-only refuse vector — same absent node. **Of §9's four remaining cases only
  `.entsite` (`C-1`) is genuinely ours and unblocked**, and most of the rest wait on
  `APP-CONVENTION-EMBED`'s node, which §9 itself says is EMBED's to ship.
  Packet: `ROUTING-2026-09-09-i-arch-F-1-CLASSIFIES-TWO-REF-FORMS-…`.
  **`G-PIN-4` IS THREE LINKS, BOTH SEATS FOUND THE SAME THREE, AND LINK 1 IS THE ONE THAT DECIDES IT.**
  `entity-workbench-go` pointed their types at our vendored emission the same day we pointed ours at
  theirs, and **corroborated both of the links we measured** (their gates run green here: 3
  `app/site-manifest`, 11 `app/site-page`, our signed root rebuilt from 15 bindings in a fresh store).
  *Source → entity* is the third and had no fixture on either side; it does now —
  `tests/fixtures/gpin4-joint/source/` (a real `render/` dir) + `EXPECTED-INGEST.json`.
  **It needed a refactor that is the real lesson: the disk→entity half was welded to a `Peers` write.**
  `ingest::read_site_dir` is pure now. *A link you cannot evaluate without standing up a peer is a link
  nobody evaluates* — and that is why the link both seats agreed was the whole risk was also the one
  neither had a fixture for.
  **§6.1's CHUNKER MUST WAS BYPASSED HERE, NOT UNEXERCISED — and the sharper claim came from checking
  their recommendation rather than adopting it. ⚠ CLOSED 2026-09-10; the pointer arm ships. Kept
  because the reasoning is what arch made normative.** They measured *"nothing either seat holds
  exercises chunking"* (right) and recommended an asset >16 KiB in the fixture. Measured through
  `make site`: a 208,046-byte source file became **ONE 208,109-byte `app/site-asset` entity**, 3 trie
  keys, no chunk list. `SiteAsset` inlined at any size, so there was no chunker on this path to
  disagree about parameters with, and a bigger file made §6.1 no more tested. That argument is now
  §4's `[MUST]` and the arm is built — see the `app/site-asset` entry below for what it cost and what
  it did not move. **Two roots remain in `EXPECTED-INGEST.json`, for a WEAKER reason than this
  paragraph gives:** our asset row is reproducible now, and what is still missing is that
  workbench-go does not model `app/site-asset` at all, so there is nothing to compare it against.
  **One root would still be wrong** — it would trade the pages-only comparand that works today for a
  full root nobody can currently match, which is the shape of a gate nobody runs.
  **A markdown page with NO frontmatter stores `title: ""`; the `.html` path deliberately removes it**
  (`page_from_html`: *"an empty title would render as a blank breadcrumb, which reads as broken"*).
  Same absence, two encodings, one module — found by building the fixture, and **pinned rather than
  repaired**: `{"title": ""}` and an absent key are different bytes, the other seat is comparing
  against ours, and a silent repair mid-comparison is the wrong shape for a two-seat decision.
  **`EXPECTED-INGEST.json` and `EXPECTED.json` are wire artifacts — regenerating either is a WIRE
  EVENT, not a test fix.**
  **Verify a counterpart's claims by running them, and say which ones you did NOT verify.** Their §5.4
  correction was right and we had already withdrawn the risk; **their ground 1 is stronger than our
  version and we took their wording** (the three-act frame is real normative text — *relaying and
  re-encoding are both claims that "this is still the sender's entity"*). **Their ground 2 we could not
  locate and therefore did not assert on their behalf.** Their `fbc2c5c`-is-stale caveat resolved with
  a diff rather than a re-cut: `format.rs` is **+21/-0** since, all additions, **no encoder changed**.
  Packet: `ROUTING-2026-09-09-j-workbench-go-LINK-1-IS-BUILT-…`.
  **`make crossimpl-site` RUNS G-PIN-4 AS A COMMAND, AND IT IS GREEN: 8 keys + the site root,
  byte-identical (2026-09-09).** `tools/crossimpl/gpin4` reads `site.json`, builds the entities
  through **entity-workbench-go's own `entitysdk`** and the trie through **entity-core-go's own
  `core/tree`**, and prints a per-key report. This is §9's *"one fixture, two publishers, identical
  site root"* in the **forward** direction — their gate measures our frozen emission *backwards*
  (decode ours, re-encode), which cannot see a field neither side emits. Falsified four ways: one
  perturbed body reds that key **and** the root and nothing else; a dropped page reds as a *key-set*
  difference with 0 keys diverging; an empty fixture is a **fatal**, not a clean report; the
  unmodified fixture passes.
  **THE COUNTERPART'S IMPLEMENTATION IS A LIBRARY ON THIS BOX, NOT A CORRESPONDENT — and that is
  the transferable half.** The entry above already says *"before opening a coordination thread,
  check whether the counterpart's artifact is already in your tree."* This is the stronger form:
  their **code** is also in your tree. `entitysdk` is a Go module fifty lines of `replace` away, so
  the answer to *"do our publishers agree"* was one command and no packet. We had instead built a
  fixture, computed our half, written a protocol for their half, and filed it — a round trip for a
  question we could answer alone in an afternoon. **Ask what you can CALL before you ask what you
  can send.** The boundary is unchanged and load-bearing: we **consume** their types, we never
  model them — the day a struct in `main.go` mirrors one of theirs, the gate measures nothing.
  Read-only `replace` paths; nothing is written into either sibling tree. **Stated host dependency,
  not hidden:** a `go` toolchain, the same call `crossimpl-go` already makes for its leg.
  **SCOPE, AND IT IS A FINDING: LINK 1 IS NOT A CONFORMANCE SURFACE, SO DO NOT BUILD A GATE ON IT.**
  The convention specifies **no authoring format at all** — no `render/` directory, no
  `site.manifest.json`, no `+++` frontmatter block anywhere in its twelve sections. §0 calls
  frontmatter *"optional local flavor not the contract"* and §4's CDDL says
  *"title-only is conformant; **MAY** derive title from first H1"*. So two conformant publishers are
  **permitted** to lower one markdown file to different entities — and the divergence our own
  fixture README predicts (`glossary` → our `frontmatter: {"title": ""}` vs an absent key) is
  **exactly** the case the spec already blesses both ways. A byte gate there reds forever,
  legitimately, and teaches the next reader to ignore it.
  **The sharpest part: that clause is OURS.** The v0.4.2 provenance line reads *"ordering tightened
  (entity-browser-rust) — … frontmatter is optional local flavor not the contract."* We argued the
  freedom into the spec, then built a fixture to compare the thing we had freed. **Grep the
  provenance of the rule you are about to test against; you may have written the exemption.**
  **`entity-workbench-go` has no source-directory site ingest** — proven, not assumed: zero TOML
  parsing, no `+++` handling, no `site.manifest.json` reader, and `git log --all -S` finds none on
  any branch. `workbench/ingest_tree.go` is a **doc-tree** ingest emitting `doc/markdown-file`, a
  different vocabulary (and the one place they *do* chunk, which is where their §6.1 note came
  from). So link 1 has no counterpart implementation to compare against, only a fixture on our side.
  **`tests/fixtures/gpin4-joint/source/` + `EXPECTED-INGEST.json` are KEPT and re-scoped, not
  withdrawn** — they pin our own ingest against silent drift and make our lowering choices legible
  if the two seats ever choose to converge. What changed is the claim: they are a **shared
  authoring note**, not *"the link that decides G-PIN-4"*, which is what the README said.
  **An anti-vacuity floor set at the REAL count turns the most interesting divergence into a rig
  fault.** `minKeys` was the fixture's true key count (8); dropping a page then hit *"anti-vacuity:
  compared 7"* and **never reached the key-set report** — *one publisher emits a key the other does
  not* is the single most valuable thing this program can find, reported as a broken harness.
  It is **2** now — a manifest and one page, i.e. *did the fixture load* — and the key-set
  comparison carries the finding. **A floor guards the rig; it must not adjudicate the subject.**
- **THE CARRIER IS ALREADY PAYLOAD-AGNOSTIC AND THE LEAK IS ONE FILE — measured 2026-09-10, against
  the opposite hypothesis.** The operator asked whether we had over-built the content model and
  under-built the payload-agnostic exchange. **The measurement inverts it.** Vocabulary references
  (`site_id`/`SiteManifest`/`SitePage`/`SiteAsset`/`app_catalog`/`app_bundle`), comments stripped, split
  at each file's `#[cfg(test)]` boundary: `signed_fetch` **0** in product (12 in tests, from line 830) ·
  `signed_root` **0** (9 in tests, from 634) · `origins` **0** · `publish_layout` **0** ·
  `foreign_cache` **2** (the `ForeignArtifact` URL dispatch, deliberate — D24) · **`http_poll` 29 in
  product (26 more in tests, `cfg(test)` at line 594), and 13 of its 21 public fns name a vocabulary.**
  So **4,395 lines of transfer layer whose public API names no payload type**, against **997 lines
  holding all the coupling**.
  **The `http_poll` cell was WRONG when first published and arch caught it — the correction is the
  transferable half.** It read **55** under a column headed *product code*, which is the **file total**,
  because the test-boundary split was run over three files and the raw count over the rest. **A table
  where one cell is computed differently from the others is worse than a table that is uniformly
  approximate**, and the tell was there to see: every other row had a `test=` figure beside it and that
  one did not. **Re-measure the whole column with one command, never per-file.** The conclusion is
  unchanged and the API-surface number — 13 of 21 public fns — was always the load-bearing one, because
  it measures the *coupling surface* rather than how many lines mention it.
  **The proof is not the API shape, it is the third consumer.** `registry_publish.rs` (1,545 lines)
  publishes **registry bindings** through the same `RootProjector::record` / `record_hash` /
  `sign_detached`, with zero site vocabulary in product code. Arch's `AT-1` records the ABI as shipped
  **twice** (`sites`, `apps`) and holds the count as an observation — *"the durable claim is the coupling
  rule, not the count."* **A third instance built for an unrelated purpose is the strongest evidence for
  the rule and against the count**, and it is routed as such.
  **Two consequences worth carrying.** (1) `http_poll`'s 13 vocabulary-named builders are **the exact
  mirror of the emit-side enumeration** this file already records (`emit_owned_sites` + `for set in
  app_sets`) — **one defect, both directions**, and a third vocabulary means editing both plus
  `ForeignArtifact` plus `resolve_publish_source` plus `run_plan`. (2) **`registry_publish.rs` already
  emits per-entity detached signatures** at the §5.2 invariant pointer — which is exactly what
  `APP-CONVENTION-FEED` §1.1 makes a MUST for entries, so that requirement is **a loop, not a design**.
  **Do not split the carrier as a standalone rename** — the charter rule stands, and FEED is the change
  that proves the boundary. Plan: `docs/plans/PLAN-2026-09-10-…`; review: `docs/plans/REVIEW-2026-09-10-…`.
- **`tools/vocab-lint.sh` IS THE APP TIER'S ONLY ORACLE, AND IT IS THE FIRST GATE HERE THAT READS
  ANOTHER REPO.** Every other linter reads our own tree, because every other failure it looks for is
  visible from inside one repo. **This one is not:** two application seats can be perfectly
  wire-conformant and completely vocabulary-divergent **with no error anywhere** —
  `APP-CONVENTION-SHARE` §2 states the shape, *a type-filtered query on the wrong tag returns a correct,
  complete, EMPTY answer.* Nothing 404s, nothing mismatches, and every test on both sides stays green.
  It wraps arch-tools' `spec vocab` (in `make lint`, baseline-ratcheted).
  **Scoped to findings that NAME us, deliberately.** `spec vocab` reports the whole tier, most of which
  we cannot fix — `app/feed/*` being declared-unimplemented is a roadmap fact, not our debt. **A gate
  that fails on another party's board is a gate people learn to skip**, so it counts only
  `implemented-undeclared` where we are a seat, `single-seat` where the seat is us, and
  `divergent-family` where we appear. The rest prints and does not gate; the full board is arch's.
  **The baseline carries NAMES, not a count** — this file already records the hole a count leaves
  (*"a SWAP passes"*) in `net-lint` and `ecf-lint`, and here a swap is the dangerous case: retire
  `app/site-asset`'s declaration debt while minting a new undeclared tag and the tier is no better off.
  Set comparison, so it **fails both ways** — a baseline row that stops being reported is also a red,
  because a baseline outliving its debt silently re-admits it.
  **It SKIPS when it cannot look and says exactly what went unchecked.** Three sibling checkouts are
  needed and a fresh clone must not fail its own lint — but a silent skip is the failure mode this repo
  has been bitten by repeatedly, so the skip names the missing path *and* the class it did not measure.
  **Falsified four ways:** mint an undeclared tag in product code → red naming it; add a baseline row
  with no live finding → red; analyzer absent → loud skip, exit 0; analyzer present but returning
  non-JSON → a *different* loud skip. Plus the positive control.
  **Two bugs it cost, both invisible to reasoning.** (1) **A heredoc and a pipe cannot both own stdin** —
  `printf '%s' "$RAW" | python3 - <<'PY'` gives python the *program* and leaves `json.load(sys.stdin)`
  reading the empty remainder, so the gate degraded to a **skip that looked legitimate**. It reads the
  JSON from the environment now. (2) **`seats` is a LIST, not a count** — `d.get("seats",0) >= 2` threw,
  and the first run reported *"fewer than 2 application seats"* on a perfectly healthy pair. Both were
  found by **running the script**, not by reading it, and both produced a green-or-skip rather than a
  red — *the two failure modes a gate must never have.*
  **IT EARNED ITS KEEP IN ONE DAY, AND ON THE EXACT CASE THE NAMES-NOT-COUNTS CHOICE WAS MADE FOR.**
  Arch answered `A-27` by **declaring** `app/site-asset`, so the finding **reclassified** —
  `implemented-undeclared` → `single-seat` — and the gate went **red on the reclassification**.
  **A count-based baseline would have passed it silently: one row out, one row in, total unchanged.**
  That is the same swap this file already records `net-lint` and `ecf-lint` as structurally unable to
  see, caught here because the baseline is a set. **When a ratchet's subject can change CLASS rather
  than only count, the baseline must name the class.**
  **SECOND INSTANCE THE NEXT DAY, and it is the stronger one because the movement was net-DOWNWARD.**
  Closing the SHARE divergence (2026-09-10) retired **two** rows — `divergent-family app/share/*` and
  `implemented-undeclared app/share/manifest` — and added **one**, `single-seat
  app/share/publication`, which is honest rather than debt we chose: arch minted the type the day
  before and we are the first seat to implement it. **A count-based ratchet would have read 4 → 3,
  passed, and never shown that the tier's only divergent family had closed** — the single most
  important thing that run had to report, invisible to a number that moved in the right direction.
  *A ratchet that only counts cannot tell you what got better.*
  ⭐⭐ **AND FOR TWO SESSIONS IT WAS READING THE KERNEL AND NAMING US — 2026-09-16, and both prior
  explanations of the row were reasoned from source, agreed with themselves, and were wrong.**
  `single-seat app/user` sat in the baseline with a long note deriving it as a doc-example false
  positive. Instrumenting `scan_seat` says otherwise: our own occurrences
  (`views/query_console/`) are classed **test-only, correctly**, and the row was produced by
  `.core-pin/<kernel-sha>/extensions/compute/src/eval/tests.rs` — **`entity-core-rust`'s own test
  file**, in five stale export directories *inside our seat*. Two independent misses compose into
  it: the analyzer's skip patterns are anchored `/target/` and tested against a path **relative to
  the seat root**, so a top-level `target/` never matches (**0 files skipped in this worktree, 1962
  build-output files scanned as our source in the other — 85% of that scan**); and `_TEST_NAME`
  matches a `tests/` *directory* or a `_test.rs` *suffix* but not a file named `tests.rs`, which is
  Rust's third idiom.
  ⛔ **THE RESIDUE OUTLIVED ITS WRITER, WHICH IS WHY EVERY INSTINCT SAID "WE FIXED THAT".**
  `core-pin.sh` moved its output to `~/.cache/entity-browser-core-pin/` on 2026-09-15 *for this exact
  reason*, and this file records the move. **The 64 MB already on disk stayed**, so the fix was true
  of the code and false of the tree — and `.gitignore` then hid the residue from `git status` while
  leaving it in plain sight of every tool that walks the directory. ⇒ ***ask what the previous
  version of a tool LEFT BEHIND, not only where the current one writes*** — and when a gate names
  you, **measure what it READ before you explain what you wrote**. Two sessions produced confident
  accounts of a finding whose cause was in neither the source nor the corpus they were reading.
  ⭐ **A LOUD SKIP IS STILL A CLASS GOING UNCHECKED, AND "RUN IT FROM THE OTHER TREE" IS NOT A
  WORKAROUND IF NOBODY EVER DOES.** The gate skipped on any worktree — correctly, since the analyzer
  resolves our seat by directory name and would otherwise report another tree's work against our
  commit — and **every branch here is developed in a worktree**, so the application tier's only
  oracle had run on **no branch work, ever**. It is CONSTRUCTED now: a scratch parent in which the
  seat name resolves to *this* checkout, materialized from what git considers part of the repo. Same
  move as `CORE_RUST_REF` one tool over, and the materialization is what closes the residue class for
  good — *ignored is what residue means*, so it needs no exclusion list and cannot rot the way
  `/target/` did when `target-pin-*` appeared. Both defects routed
  (`ROUTING-2026-09-16-b-arch-…`), with the half of our own prior packet that measurement falsified
  withdrawn as `C-3`: we had told arch a sample string was read as an emission, which is a claim
  about a repository, where the analyzer reads a **directory**. *A sentence true of your repo and
  false of the directory a tool walks is the shape this whole entry is about.*
- **A JOINT FIXTURE'S COMPARAND IS CONVENTION-SPECIFIC, AND TWO OF OURS GIVE OPPOSITE ANSWERS ON THE
  SAME AXIS — do not copy `gpin4-joint`'s README onto the next one (2026-09-12).** That file says, in
  as many words, *"the peer id and the keypair do not matter"*, which is true of a site and **false of
  a feed in both halves, silently**: `FEED-R1` makes an entry's `author` equal the namespace it is
  read under and every index page is a list of `pin(author, hash)`, so **the peer id is in the bytes**.
  `SHARE` is the mirror image — §4 makes the *namespace* the publisher, `decode_share` takes it as an
  argument, and **no peer id appears in any share body at all**. ⇒ *ask which of the two you are in
  before designing a fixture for either.*
  **Pin a SEED, not a peer id, whenever the id is in the bytes.** A literal id makes the bodies
  comparable and leaves the per-entry detached signature out of the comparison entirely — you cannot
  sign as a peer whose key you do not hold. Ed25519 is deterministic, so a seed makes the id, the
  identity entity **and every signature byte** reproducible, and `FEED-R2` becomes checkable rather
  than aspirational (`tests/fixtures/feed-joint/`, `src/feed_joint_fixture.rs`, `J-4`).
  **And only some keys are the convention's.** FEED §4.2 pins the index paths by hand; §2 says the
  cross-impl contract is the **type tag, not the path**, so an entry's key is ours — hence **two
  roots**, `index_root` (theirs to match) and `feed_root_ours` (our own drift detector). Same split as
  the site fixture's two roots for a *different* reason: there the second waits on a **type** the
  other seat has not built, here on a **path** the convention declines to pin. **Say *reproducible*
  and *comparable* separately.**
  Three smaller things that cost something: the signature goes through the production emitter
  (`feed_publish::signature_entity`) rather than a second expression of *how this system signs one
  entity* — **a fixture is the most tempting place to write one** (C15); §1.1's shorthand *"`signer =
  author`"* assigns a field in the **kernel's** type whose own doc shouts *NOT the peer-id string*, so
  **open the defining document when a convention assigns a field it did not define**; and the report
  is **per-key before any root**, which is the one thing the other seat asked back for.
  **The regenerate spelling is containerized** — `make test-one T=<name> EXTRA_RUN_ENV="-e
  <X>_REGENERATE=1"`. `gpin4-joint`'s README documented a bare host `cargo test` that **dies in
  `openssl-sys` on a podman-only box**; corrected the day the second fixture needed the same command
  (AP37 — *run it before you write it down*).
- **A JUSTIFICATION STATED OVER A WHOLE PREDICATE DOES NOT TRANSFER TO EACH CLAUSE OF IT — and the
  cheap tell is to delete the clause in a test (2026-09-12, `A-24`/`B-10`).** We told arch in **three**
  packets that refusing `REFERENCE` §3.2's `path` form at the asset position is *"a security property —
  subgraph confinement"*. True of `asset_name_from_ref` **as a whole**; **false of the leading-slash
  clause**, which is the one the ruling turns on. Measured: an asset ref has exactly **one** base and
  it is the **site root** (`asset_path` = `/{peer}/sites/{site}/assets/{name}`; no page-relative
  resolution in either of the two callers), so `/assets/x.png` and `assets/x.png` denote the same
  bytes and the refusal is a **pure loss**. What confines the subgraph is the mandatory `assets/`
  prefix plus `..`/`//`/`://`/`data:`.
  **Nothing about the sentence was false; the inference was** — and it had been load-bearing in
  somebody else's open ruling for three days. ⇒ **ask which clause would have to be removed for the
  claimed harm to become reachable, and remove it in a test.** Four minutes.
  `the_leading_slash_refusal_is_not_what_confines_the_subgraph` (`paths.rs`) is that test: it runs
  every hostile input through a *hypothetical* single-slash strip and asserts each is still refused,
  so the analysis is a measurement and a later session cannot re-derive it backwards.
  ⭐ **`entity-workbench-go` found it, and the useful move was the third option: VERIFY THEIR CLAIM IN
  YOUR OWN SOURCE** — not accept it, not dispute it. Their wording was better than ours
  (`workbench/site_ref.go:299`) and checking it cost one function read.
  ⛔ **Behaviour deliberately unchanged**, because site-root-relative vs peer-relative is *precisely*
  what `A-24` leaves unruled: widening would make us a second seat asserting a reading of a clause we
  had just found we mis-argued. **A ruling made on a premise you now know is partly wrong is worse
  than a delayed one** — so the correction is what is owed, not the change. It also **fails safe** on
  our side: a refused ref renders no figure, never somebody else's.
- **WHEN A NORMATIVE FIELD'S TYPE HAS NO DEFINITION, GREP FOR THE PRODUCTION NAME — if it resolves
  nowhere, THAT is the finding (2026-09-11).** We asked arch what an `app/feed/entry`'s `body` should
  contain, reasoning that `body: embed-node` needed an `Embed` entity and **neither seat has one**. The
  premise was a level below where either seat looked: **`embed-node` was defined nowhere.** One
  occurrence in all of `specs/` and `guides/` — in `APP-CONVENTION-FEED` §2.3's own CDDL, *as the type
  of the field*. EMBED defines `embed-data` and the `Embed` entity and **no production of that name**.
  We were not blocked on an absent entity; **we were reading a dangling reference**, which is why the
  question had no good answer and why every answer we reasoned toward was wrong in an interesting way.
  **The near-miss is why it mattered.** The term's only descriptive use in the corpus points at
  **`EmbedOutput`** — EMBED §4, the *output* contract — and a fixture built to that description would
  have **published rendered output as the stored wire format**, breaking three `[LOCKED]` sections at
  once and, worst, **§4.0 makes `EmbedOutput` a CLOSED vocabulary by design**, so the format could never
  carry a content kind that vocabulary did not anticipate. The input surface is open on purpose. **One
  fixture from publishing the closed half of the model as the tier's baseline**, against our own
  *whatever publishes first is the baseline* rule.
  **This is the third blocker in three days that dissolved on opening the corpus, and the shape is
  new** — the first two were answers in a section nobody opened; this one was **a production that does
  not exist**. Resolved as EMBED §3.1, the inline `(type, data)` form: *a node and an entity differ in
  addressing only, and the dispatch key is carried rather than dropped.*
  ⭐ **THE OTHER OUTCOME, 2026-09-14, and it is the commoner one: IT RESOLVES SOMEWHERE, WITH MORE
  ARGUMENTS.** FEED §6.0.1's live coordinate read `hex(content_hash(absolute-path))` — a **one**-argument
  spelling of a function the corpus defines with **two** (V7 §1.4, over `{data, type}`) — so we filed
  `A-60`, invented `sha256(utf8(absolute))` on the ground that its input was the only one fully
  determined by the clause's own words, and wrote in the doc comment that one function moves whichever
  way it was ruled. **The reading we could not derive was not an invention: it is `EXTENSION-REVISION`
  §3.1's landed `prefix_hash`, and the missing type tag is `system/tree/path`, a value this corpus
  already ships.** ⇒ **when a clause names a function with the wrong arity, grep the whole corpus for
  the production name before deciding which argument to invent** — the missing argument is usually
  pinned in a sibling document, and the arity mismatch is the tell that you are reading a *citation*
  rather than a definition. Fifth instance of *a blocker that asks for more coordination is the one to
  re-read the corpus about*, and the first where the missing piece was a **function** rather than a
  sentence. **And CALL it rather than restating it** — `prefix_hash` is `[derive-to-meet]`, so two seats
  computing it separately must land on the same byte with **nothing failing loudly** if they do not
  (two conformant peers simply construct different paths and never meet). That is C15's drift shape with
  a wire event on the far end, which is why `entity-revision` is a direct dependency for one function.
- **`app/site-asset` TAKES EMBED §3's POINTER ARM ABOVE 16 KiB — shipped 2026-09-10
  (`content_site/asset_store.rs`), and it was NOT a fixture-conformance item.**
  `SiteAsset` was `{media_type, bytes}`, inline at any size; it is now `{media_type, payload}` over
  EMBED §3's **tagged** union, reused rather than restated (the §4 CDDL says so: *"NOT a second
  payload union; the embed one, reused"*). **We had the reasoning half-right and arch made it
  normative:** inlining does not *fail* §6.1's canonical chunking MUST, it makes it **unreachable** —
  *"no reproducible-publish check can observe it."*
  **THE HANDOFF SCOPED THIS OFF THE FIXTURE'S 32 KB `big.svg`, AND THE FIXTURE IS THE SMALL HALF.**
  Measured on a real published tree before touching anything: **629 of 1,346 site assets are over 16
  KiB (47%)**, largest 476 KB — i.e. most figures in the papers sites. So a producer-only change was
  never shippable; it would have blanked half the figures on the live sites. **Before pricing a
  conformance fix, measure how much production traffic sits in the governed case** — the fixture is
  where you *notice* the rule, not where its cost lives.
  **`stage` RETURNS the blob closure rather than only writing it.** Chunking leaves every caller a
  second obligation — the publisher must project the blob+chunks, the browser must persist them — and
  a function that only wrote into the store it was handed would leave that to memory (AP44). A caller
  that ignores the return gets an unused-variable warning, not a site whose figures 404. Same reason
  `read::OwnedSite` and `resolver::ResolvedPage` grew a `content` field: the site travels whole.
  **`--verify` WAS GREEN ON A TREE WHOSE FIGURE NOBODY COULD RESOLVE, AND THAT IS AUDIT F8 A SECOND
  TIME.** Measured by hand, not reasoned: delete the asset's chunk from a published tree and verify
  printed *"every pointer resolves and every body hashes to its address"*, exit 0, while the closure
  count silently dropped 11 → 10. `trie_closure` finds a `payload.hash` fine — a 33-byte hash is a
  33-byte hash wherever it sits — but it **filters on presence, so it cannot see absence**. The fix
  was already written down *in that same function* by F8: *"A heuristic scan cannot close it…
  Structure can."* An `app/site-asset` declares its blob and a `system/content/blob` declares its
  chunks, so both are decoded now. **Worse than a missing trie node and that is why it got its own
  gate** — a missing interior node makes a subtree unreadable and is loud; a missing asset closure
  renders the page perfectly and drops only the image. Gate:
  `a_pointer_assets_blob_closure_is_declared_so_verify_fails_when_it_is_missing`, **both arms
  falsified separately** (drop the asset arm, drop the blob arm).
  **THE WIRE EVENT, and what it did NOT move.** `EXPECTED-INGEST.json` was regenerated —
  `site_root_full` and the asset binding moved; **`site_root_pages` did not**, and neither did
  `EXPECTED.json`, so `make crossimpl-site` still reports **AGREED** and the comparand the other seat
  actually runs against is untouched. **The plan predicted the two roots would collapse into one.
  They do not, and collapsing them would have been a regression:** `entity-workbench-go` does not
  model `app/site-asset` at all, so one root would trade the pages-only comparand that works today
  for a full root nobody can compare against. What changed is *which side the gap is on* — our bytes
  are now reproducible from the source file and the canonical chunker alone; the missing piece is a
  counterpart implementation. **Say "reproducible" and "comparable" separately; they are not the same
  claim.**
  **The boundary is asserted from both sides** (`the_inline_ceiling_is_asserted_from_both_sides`):
  16,384 is the *last* conformant inline size, 16,385 the first pointer size. `F-5`'s lesson — a
  bound asserted from one side passes for an implementation that only ever went one way, and here
  each direction has a different wrong implementation (pointer-everything vs the all-inline path this
  change retires).
  **Worker-arm bound, stated:** resolving a pointer needs a content-store read by hash, the renderer's
  asset resolver is a **synchronous** closure inside a render pass, and the Worker/OPFS proxy has no
  content verb — so `?worker=1` does not hold an oversized asset's bytes locally. Direct/IDB is the
  shipped default. `WriterHandle::content_put`/`content_get` is the seam and it **logs** on Worker
  rather than silently succeeding, for the reason that module's own history gives.
  **Pre-existing Makefile defect found on the way, NOT fixed here:** `stage_publish_sources` is a
  no-op when `VERIFY` is set (`$(if $(VERIFY),,…)`), so `make site INGEST=… VERIFY=1` publishes
  **nothing** — `--ingest=.ingest-stage` names a directory that was never created, and the run exits 1
  from verify complaining the tree is absent. Confirmed by neutering the whole change (`git stash`)
  and reproducing identically: *if the symptom survives your change being gone, the symptom is not
  yours.* Publish and verify as two invocations until someone fixes it.
- **CHANGING A WIRE SHAPE IS A QUESTION ABOUT THE BYTES ALREADY PUBLISHED, AND THE POINTER ARM
  SHIPPED WITHOUT ASKING IT — found in the audit of the commit that shipped it, 2026-09-10.**
  `SiteAsset::from_entity` read only the new `payload` key, so **every already-published asset
  decoded to `Inline(0 bytes)`**. Measured on a real 30,778-byte published figure:
  `resolve()` → **`Ok(0)`**, rendered as `data:image/png;base64,` with no data. **All three gates
  were green** — the suite, `crossimpl-site`, and `e2e-worker` 68/0 — because every fixture in every
  one of them was written by the new encoder. *A test population you generated cannot contain the
  shape you are migrating from.*
  **What makes it a blocker rather than a migration note is the cache-arm ordering, and that is the
  transferable half.** `MultiResolver::resolve_page` tries the durable cache **FIRST, with no
  network**, so a returning visitor to any foreign site they have already viewed reads the old-shape
  entity out of their own tree and **never issues the fetch that would heal it**. Republishing the
  origin does not reach them. So the usual reassurance — *"it fixes itself on the next publish"* —
  was false here, and checking *which arm serves the stale copy* is what turned a "we should
  migrate" into "we cannot ship this".
  **Two defects, one commit, and only one of them is about compatibility.**
  **(1) The silent-empty default.** `AssetPayload::default()` was `Inline(Vec::new())`, so *"we
  cannot read this"* and *"the publisher shipped an empty file"* were **the same value on the
  SUCCESS path** — AP40 where no care at a call site can recover it. Sharper still:
  `inline-payload` is `bstr .size (1..16384)`, so **`Inline(vec![])` is not representable on the
  wire at all** — an illegal state was being used as the safe default. Now `AssetPayload::Unreadable`
  + `ResolveError::NoPayload`. **When you pick a `Default` for a decoded wire type, check the CDDL
  admits it.**
  **(2) The legacy read**, deliberately narrow: it fires **only when `payload` is absent entirely**,
  never when it is present and unreadable, so an open type growing a `bytes` key meaning something
  else cannot be claimed by it. It is a **read, never a write** — `to_entity` emits only the declared
  shape, so the cache write-through heals as it goes — and it is **reported** through
  `SiteAsset::from_legacy_encoding` at the two one-shot boundaries (HTTP fetch, owned-tree read) and
  **never in the render loop**, because a papers page has twenty figures and twenty warnings a frame
  is how a real signal gets muted. *A concession with no instrument behind it can never be retired.*
  **The same collapse a second time, found by reading the CDDL instead of our own comment.** Our
  decoder mapped every unknown payload tag to `Unreadable`, i.e. *malformed*. But the site-asset
  CDDL says `payload: embed-payload`, and that union is `inline / pointer / **child**` — so a
  **conformant** producer can emit an arm we simply have not built, and calling their bytes corrupt
  is the wrong attribution. Split into `AssetPayload::Unsupported { tag }` /
  `ResolveError::UnsupportedPayload`. **An UNTAGGED payload is still refused** (EMBED §3's MUST) —
  *unknown tag* and *no tag* are different facts and stay different.
  **And the zero-byte edge, from the same re-read:** `stage` used `<= MAX`, so an empty file emitted
  a non-conformant `.size (1..16384)` payload. The range is `1..=MAX` and **everything outside it —
  above or below — takes the pointer arm**, which represents an empty blob exactly. *Read the bound
  as a RANGE; a ceiling has two ends.*
  **The consequence for the cache is a rule worth carrying:** `persist_to_cache` stores
  `asset.to_entity()`, a **re-encode from the decoded struct, not the origin's bytes** — so an arm we
  did not understand would be dropped on the way in and the cached copy would claim to be a complete
  asset carrying nothing. It now **skips** any asset it could not use, leaving no entry so the next
  visit re-fetches. **If your cache re-encodes rather than storing bytes, it can only cache what it
  fully understands.**
  Gates: `an_asset_published_before_the_pointer_arm_still_resolves_to_its_bytes`,
  `an_unreadable_payload_is_an_error_and_never_an_empty_success` (three shapes, three *distinct*
  answers — an earlier cut asserted one answer for all three and had to be corrected),
  `a_present_but_unreadable_payload_does_not_fall_back_to_the_legacy_key`,
  `re_encoding_a_legacy_asset_emits_the_declared_shape`,
  `an_empty_asset_is_outside_the_inline_range_and_takes_the_pointer_arm`. Both neuters falsified and
  they hit **different** test pairs, so the legacy read and the `Unreadable` default are separately
  measured. **The audit moved no wire bytes** — `EXPECTED-INGEST.json` is byte-identical after it, so
  the packet already routed to `entity-workbench-go` stayed accurate.
  **Standing check, and it is the one this arc says to add:** *when you change a stored shape, decode
  something a PREVIOUS BUILD wrote before you believe any gate.* A published tree is on disk in
  `dist-*`; it costs one probe test. And ask which arm serves a stale copy — if a cache is consulted
  before the network, "the next publish fixes it" is not true.
- **A TAG CORRECTION IS A BODY QUESTION — the SHARE divergence, closed 2026-09-10 (`src/share.rs`).**
  We emitted `app/share/manifest`; `APP-CONVENTION-SHARE` §2 has declared `app/share/record` since
  v0.1. That was the application tier's **only** `divergent-family` — two seats, **zero tags in
  common** — and the failure is the one §2 names: *a type-filtered query on the wrong tag returns a
  correct, complete, **empty** answer.* Nothing 404s, nothing mismatches, every test on both sides
  stays green. Ours was the wrong tag, so we moved.
  **It read as a rename on every board that tracked it, including ours, and it was not.** The body
  diverged on **four** axes at once: audience was a token string where §2.2 wants
  `[* audience-entry]`, `target` was flat sibling `blob`/`prefix` keys with the variant **inferred
  from which one was present** (§2.2's CDDL rules that out by hand — *"TAGGED — no untagged
  ambiguity"*), the publisher was a self-declared `from` field, and `created_at` was absent while the
  schema requires it. **Shipping the ruled tag over the old body is strictly worse than the honest
  divergence**, because the failure moves from *"we cannot find your shares"* to *"your shares are
  corrupt"* — entities that **claim** conformance and fail to decode at a conformant reader. The
  other seat named this trap for us before we started (`WC-2`), which is the argument for stating a
  correction you owe *before* you make it.
  **INTEROPERABLE AND BYTE-STABLE ARE DIFFERENT CLAIMS, and the second is what decides extension
  fields.** V7 §2.6 makes unknown fields MUST-ignore, so keeping our `kind`/`from`/`size` would have
  interoperated perfectly — and **SHARE-1/SHARE-2 assert the record encoding is byte-stable
  cross-impl**, which an encoder that always adds a key of its own can never satisfy. So the rule is
  **emit the declared fields and nothing else**, and it is `G-PIN-4`'s lesson arriving one convention
  early: whatever publishes first is the baseline. Sibling to *reproducible ≠ comparable* one entry up.
  **DROPPING A PROVENANCE FIELD WAS A CORRECTNESS GAIN, NOT A COST — the general form is worth
  carrying.** §4 requires a mirrored share to sit at `/{publisher}/…` verbatim, so the publisher is
  **always in the path**. That makes the namespace a **fact** where a `from` field is *a stranger's
  self-declared claim about their own identity* — the decoder takes the publisher as an argument and
  a body that smuggles one back cannot override it (gated). **When the structure already carries a
  fact, a field restating it is not redundancy, it is an untrusted second source** that a reader has
  to decide between. This is D25's axis pointed the other way: there, a value needed provenance
  added; here, provenance was already load-bearing in the path and the field was the weak copy.
  **§2.6's MUST-IGNORE IS THE DEFAULT THAT FIGHTS THE CARVE-OUT — AP45's shape, second convention.**
  SHARE-8 requires a `publication` carrying an `audience` to be **rejected as invalid**. Follow §2.6
  and you skip it silently, laundering the one shape the type exists to exclude into an
  ordinary-looking row, with nothing anywhere reporting it. Refused as its own outcome
  (`InvalidForType`, kept apart from *malformed* — a deliberate schema violation is not a corrupt
  byte). Same trap `GUIDE-ENTITY-WORKBENCH-APP` §5.4 rule 3 caught us on for two months: **when a
  rule carves an exception out of a rule you satisfy, the exception needs its own handling and its
  own test.** Two conventions now; a third makes it tier-wide rather than local.
  **`Public` and *self-only* are SIBLING VARIANTS, not two values of one field**, because SHARE-7
  exists precisely because reading an empty audience as public is the intuitive-and-wrong answer —
  and a type that cannot express the confusion cannot ship it. `policy_key` → **`policy_keys`**
  because §3 is plural (one `audience-entry` per member, each with its own minted token); the
  singular version could only ever express one.
  **Verified in the kernel rather than reasoned, and it corrected the reasoning:** a publication
  still files under the `default` policy key. `default_connection_grants()` covers only
  `system/type/*` + `system/handler/*` reads, `capability:request` and `observe-address` — **not
  `system/content`** — and `assemble_inbound_grants` UNIONS the matched policy entry on top, so
  `default` is *how* §2.5's *"none required, pull-only"* is delivered rather than a contradiction of
  its *"no grant"*. **The same `default` key is a CEILING on the `request` path and a FLOOR on the
  connection path** (the kernel says so in its own comment, and records that the collision cost a
  13/13 → 7 P/6 F regression) — which is exactly why the sentence has two readings. Routed as `A-31`
  with SHARE-9 as the deciding argument: a vector requiring a consumer with **no token** to be **not
  refused** is unpassable under the literal reading.
  Gates: 32 in `share.rs`, **four neuters, each landing on distinct tests** — tolerate an audience on
  a publication (reds printing the laundered `audience: Public` on the *success* path) · read an
  empty audience as public · emit `from` / trust the body's `from` (**red separately**, the second
  showing `"SOMEONE-ELSE"` beating the real publisher) · a tolerant legacy read of the old tag (reds
  with the asset regression's exact shape, `Ok(Share { title: "", target: Prefix("") })`).
  **No legacy read, and the discriminating question is the entry above's:** *which arm serves the
  stale copy?* **Here, none** — nothing in the product decodes a share entity yet, the live
  file-transfer listing reads `file_offer`'s own untouched manifest, and `withdraw_share` is
  path-based. So an old row is **inert**, not stale: refused as `NotAShare`, contributing no policy
  key, never a silent empty success. A legacy read would also keep the retired tag in product code,
  which is the debt the change exists to pay. **Gated anyway**, from a hand-built fixture in the old
  encoder's literal layout — the asset regression's root cause was that every fixture in every gate
  had been written by the *new* encoder, so a fixture produced by calling this module would rebuild
  the hole.
  **Stated bound: a blob share's size is no longer persisted.** Recoverable from the content store;
  the surface that wants it looks it up.
- **WE STATED A RULE AND PROPOSED BREAKING IT IN THE SAME PACKET — A-30b, caught by arch 2026-09-10.**
  The SHARE work established, in as many words, that *a deliberate schema violation is not a corrupt
  byte* and kept `InvalidForType` apart from *malformed* for SHARE-8. **Four sections later the same
  document proposed that if arch answered "no" on A-30, `child` would move from
  `UnsupportedPayload` into EMBED §3's untagged refusal and *"nothing else changes"*.** That is the
  identical collapse, in the identical direction, against the identical rule — proposed by the author
  of the rule, about a neighbouring convention, in one sitting.
  **EMBED §3's refusal governs a payload with NO DISCRIMINATOR.** A `child` payload on an
  `app/site-asset` is correctly tagged, well-formed and unambiguous; it is invalid *for its type*.
  Reporting it as malformed tells an operator the publisher shipped a corrupt byte when they shipped a
  schema violation — **and those route to different people.** There are **four** outcomes and each
  names whose defect it is: resolved · *a tag we have not built* (**ours**) · *`child` on an asset*
  (**theirs, a schema violation**) · *no tag at all* (**theirs, malformed**).
  **The transferable half is not the rule — it is that having the rule did not help.** A principle
  articulated about subsystem A does not fire when you are reasoning about subsystem B, even minutes
  later, because the *shape* is what recurs and the shape is not what you indexed it under. **When you
  write down a distinction, grep your own open proposals for the collapse it forbids** — the cost here
  was zero only because a counterpart read both halves of one document.
  **And the argument we used was the weaker one available.** We reasoned from SITE §4's preamble —
  *an asset is bytes with a name* — and honestly flagged it as an inference from prose. The version
  that does not rest on prose was one document over: `child-payload.ref` names a sibling `Embed`, and
  **an `Embed`'s dispatch key IS its type tag**, which is precisely why EMBED §3 carries no
  `data.media_type` (*"a redundant second source of truth"*). A site asset carries `media_type` in
  `data`. **So a `child`-payload asset holds two media types that can disagree** — the exact redundancy
  EMBED deleted by hand, reintroduced one convention over. *When your argument for a narrowing is
  "the prose reads that way", go and look for the structural version in the document you are
  importing from.* Gate: the four outcomes in
  `an_unreadable_payload_is_an_error_and_never_an_empty_success`, which asserts the **discrimination**
  (SITE §9's vector fails a run that reports `child` as malformed *or* as an unimplemented arm).
  Falsified.
  **Third instance of the carve-out rule, and it generalised rather than repeating.** The first two
  (`GUIDE-ENTITY-WORKBENCH-APP` §5.4 rule 3, SHARE-8) are about **unknown fields**; this one has no
  unknown field in it — `app/site-asset` **imports a union wider than the importing field admits**, so
  a decoder reusing the imported type accepts the excluded arm and reports nothing. Same laundering,
  different mechanism. It is now tier-wide in `GUIDE-APPLICATION-DEVELOPMENT` §3's binding table:
  ***where your type carves an exception out of a rule you otherwise satisfy, the exception needs its
  own refusal and its own named check.*** We predicted the promotion trigger (*"if a third appears it
  is probably a tier-wide note"*) and then supplied the third ourselves, in the same packet pair.
- **PHASE 2a IS BUILT — the reference atom, the embed node and four FEED types — AND BUILDING THREE
  IMPORTED CONVENTIONS FOUND SIX THINGS READING THEM DID NOT (2026-09-10).** `src/entity_ref.rs`
  (`APP-CONVENTION-REFERENCE` §2.1's `entity-ref` + §3's string form) · `src/embed.rs` (EMBED §3's
  `embed-node`, `embed-data` and the payload union) · `src/feed.rs` (`app/feed/{entry, index-head,
  index-page, follow}`) · `src/percent.rs`. **None of it is on the wire yet** — no type carrying an
  atom ships, and the FEED types have no publisher until 2b.
  **Order was reference → embed → feed and it is not arbitrary:** a convention that IMPORTS an atom
  cannot be implemented before the atom has a home, and EMBED's `child` arm was literally unwritable
  until `entity-ref` existed. **The handoff's own sizing (*"~600 lines by analogy with the site
  vocabulary"*) did not hold, and the reason generalises: the site convention imports no atoms and
  FEED imports two.** Count what a convention IMPORTS before you price it by the size of a sibling.
  **THE ONE THAT COSTS SOMETHING IF IT IS MISSED: `REFERENCE` §4's `[MUST NOT]` binds *"a producer
  **that emits the atom at all**"*, and we emit `entity://` in link positions today** (`content_site::
  location`, the static export, the content-site output). We emit no atom, so we are conformant **now**
  — and the clause engages the moment a type carrying one ships, which is the whole of 2b. The consumer
  half is already right (§4's SHOULD keeps `entity://` resolving). **Routed as a scope question — what
  is the granularity of *"a producer"*: the implementation, the document, or the link position? —
  because the answer decides whether a published-corpus migration is owed.** *A conditional MUST NOT is
  a tripwire with your own next commit on the other end; go and find yours before you trip it.*
  **§3.2's losslessness `[MUST]` names no encoding for three of the four query/fragment terms, and for
  two shapes it is unsatisfiable rather than merely unspecified.** Satisfying it *jointly* — the only
  way it means anything — needs a spelling for `via`, for the `at` fragment and for the **parameter
  order** (byte-identical re-serialization requires a canonical one). `hash`/`seen` we did **not**
  invent: `Hash::to_hex` is V7 §3.5's invariant-pointer spelling, already published by this system —
  *reach for the spelling the corpus already emits before you mint one.* And: `via: []` and
  `at: {field: []}` have **no** string spelling (zero query occurrences *is* absent), and a
  peer-relative `path` cannot survive a URI path component that always begins with `/`. We collapse the
  first two **in the encoder as well as the decoder** so the wire cannot express what the string cannot,
  and normalize the third on construction. Every choice is stated in the module doc and pinned by a
  test, so a ruling moves one file.
  **`RefError::ExcludedField` is the carve-out rule applied at the first opportunity after it went
  tier-wide** — §3.1 refuses a *string* carrying both `hash` and a non-empty path while the *atom* form
  is silent about a `"pin"` carrying `path`, and §2.6's MUST-ignore would launder the confusion §2.3
  spends a paragraph excluding. Refused, kept apart from `Malformed`, **narrow** (the discriminator's
  own two slots), and `an_ordinary_unknown_field_is_still_ignored` is the arm that measures the
  narrowness — verified to stay **green** under the neuter that reds the refusal.
  **EMBED §3's PAYLOAD UNION WAS ALREADY IMPLEMENTED, ONCE, IN THE WRONG PLACE — and its own comment
  said so while the code did not.** It was `content_site::format::AssetPayload`: named for assets,
  codec-private, carrying a union that belongs to EMBED — and the file's CDDL note read *"NOT a second
  payload union; the embed one, reused"* over code that restated it. C15's shape, caught **before** the
  second carrier rather than after. **A comment claiming a factoring is not the factoring**, and the
  tell is cheap: ask which module would have to change if the shared thing changed. Moved with
  `EXPECTED-INGEST.json` byte-identical and `make crossimpl-site` AGREED at the unchanged root as the
  one-command proof.
  **The move created a decode path that did not exist and it needed a row:** the shared decoder
  *parses* the `child` reference, so a `child` now reaches the site layer as **`Ok(Child)`** and as
  **`Err(BadRef)`**, where before there was one path. Both land on `InvalidForType` — the field refuses
  the arm whatever its reference says, and adjudicating an atom we would never follow would report the
  publisher's smaller mistake and hide the one that decides the outcome. **The existing vector's `child`
  row carried a deliberately-too-short hash, so it exercised only the `BadRef` path**; the row with a
  reference that *parses* is what reds under the `Ok(Child)` neuter. *When a refactor adds a second
  route to an existing outcome, the old vector covers one of them.*
  **⚠ EMBED §3 CARRIES A MUST WHOSE JUSTIFICATION NAMES THIS REPO, AND WE DO NOT SATISFY IT.** *"An
  inline directive MUST lower to a `child` payload; it is sugar, not a parallel format
  (workbench-go/entity-browser-rust round-trip pin: 'edit in entity-browser-rust, view in workbench'
  requires the directive and the child entity be the same thing)."* **Ours lowers to nothing** —
  `content_site::embed`'s directive carries a site-relative asset path resolved against
  `app/site-asset`, we mint no `Embed` entity, so there is no `child` payload for it to *be*, and what
  we ship is the parallel format that sentence excludes. It is the sharp form of `A-24`. **No gate can
  see it: a MISSING emission is not a divergent tag**, so `vocab-lint` is silent by construction.
  Recorded in both modules' docs; not repaired, because the repair is a design question about whether
  SITE's asset model becomes `Embed` entities.
  **FEED §2.4 AND §4.4 DECLARE TWO DIFFERENT CURSORS, AND THE DECLARED ONE CANNOT SATISFY `FEED-R14`.**
  §2.4's CDDL is `? cursor: content-hash`; §4.4 says *"the cursor is `{page, applied}`"* and, in as many
  words, *"this is why the cursor carries a number and not only a hash"*. A bare hash is exactly the
  form §4.4 rules insufficient, so *resume from `page` when `applied` no longer resolves* has no field
  to read `page` out of. **We implement the DECLARED field and route it** — `G-PIN-4`'s rule, one
  convention early: whatever publishes first is the baseline, and inventing a `{page, applied}` map
  would make us the baseline for a shape the spec does not declare. Bounded and stated: §2.4 makes a
  follow record **the reader's private data** that nothing publishes, so where the page number lives for
  our own resumption is a phase-3 local-state decision. **Pinned by a test, not only by prose**, so the
  answer has a gate to change.
  **An entry KEEPS `author` where a Share DROPPED `from`, and that looks like an inconsistency between
  two conventions and is not.** A share is read in the publisher's tree, so §4 of SHARE makes the
  namespace the fact and a `from` field a stranger's self-declared claim about their own identity — we
  deleted it. **An entry TRAVELS** (§6's mirror), and one read out of a gatherer's tree has no namespace
  to consult. `FEED-R1` keeps it honest and `from_entity` takes the namespace and refuses a mismatch.
  ***Carry a fact the structure supplies only where the structure stops supplying it.***
  **Two requirements are asserted as ABSENCES, which is the only way to assert them.** `FEED-R8` forbids
  rejecting an entry for an implausible timestamp and `FEED-R12` forbids assuming any page size — both
  are things we must NOT do, and *"we do not check X"* is exactly the claim a later author repairs into
  a bug. `0`, `u64::MAX` and a 500-entry page are pinned.
  **A NEUTER CAME BACK GREEN FROM THE THIRD CAUSE, AND THE SHAPE IS NEW.** The charter already records
  two (the gate does not measure it; the neuter did not land) and a third (*the thing you neutered does
  not do what you thought*). This is a fourth, inside the assertion: `an_empty_via_…` compared
  `with_empties.to_value()` against `bare.to_value()` — **two atoms against each other** — so an encoder
  emitting `via: []` for *both* satisfied it while the property the test's own name claimed was *absent*. It
  asserts the key is **not in the encoded map** now and reds alone under the same neuter. ***When a test
  asserts two things encode alike, ask whether the property you mean is "alike" or "absent"*** — an
  equality between two outputs of the same defect is satisfied by the defect.
  **`vocab-lint` WENT RED CORRECTLY (four honest `single-seat` rows — both seats measured at ZERO on
  FEED and we are the first to implement any of it) AND CANNOT SEE A PARAMETRIC FAMILY BEING
  IMPLEMENTED.** `spec vocab` reports `embed` as **3 declared / 0 implemented** while this repo now
  ships an `app/embed/{media_type}` encoder. The mechanism is in the analyzer's own source and its rule
  is individually correct — a literal ending in `/` is skipped, because `"app/share/records/"` is a tree
  **prefix** and the first run of that analyzer reported a path as undeclared vocabulary. But
  `"app/embed/"` is not a prefix, it is the parametric tag the convention declares, and the concrete
  tags are **composed at runtime** and are literals nowhere. **The declaring side has `_DECL_PARAM` for
  parametric families; the implementing side has no counterpart.** Direction matters: `_DECL_PARAM`'s
  own comment says the expensive false direction is *accusing a conformant seat of inventing
  vocabulary*, and **this is the quiet one — it under-reports in silence**, the same shape as a missing
  emission not being a divergent tag. Routed as an analyzer observation; nothing for the baseline to
  hold, because **the finding is an absence of findings**.
  **Scope, stated so nobody reads it wider:** four of FEED's six types. `collection` (§5) and `mirror`
  (§6) are **not** built — the mirror is phase 4 and needs the byte-fidelity republication path rather
  than a codec, the collection has no consumer — and `FEED-R2`'s per-entry detached signature is **2b's
  loop, not this module's** (the path builder is the kernel's `invariant_signature_path`, the emitter is
  `registry_publish::sign_detached`). **There is no resolver**, so every `REFERENCE` requirement phrased
  *"a reader that resolves…"* — `REF-R5`, `R6`, `R19`, `R21`, and vectors `REF-V7`/`V11` — is
  **unmeasured**. Do not read these green tests as those being covered.
- **PHASE 2b IS THE LOOP, NOT `FEED-R2` — `src/feed_publish.rs`, 2026-09-10, and closing it found
  five things three phases of codec could not.** Publish entries + their detached signatures + the
  §4 index through the **same `RootProjector` a site publish uses**, and read them back through the
  **same `SignedSession` a site consumer uses**, with `FEED-R4`'s attribution attached. `make test`
  1699 → **1713 / 0 / 19**, `make lint` green, `make crossimpl-site` **AGREED at the unchanged
  root**. Native only. ⚠ **The "no CLI verb publishes a feed" half is RETIRED as of 2026-09-10** —
  `publish --ingest-feed=<dir>` does, through the axis table.
  **The handoff scoped 2b as "mint the signature entity", which is two dozen lines and would have
  left THREE CONSECUTIVE PHASES OF CODEC WITH NO CONSUMER.** That is AP34/AP35's population problem
  arriving by a different road: a module's own round-trip test cannot see anything that lives
  *between* modules, and every finding below is in that gap. ***When a phase's deliverable is a
  third codec, ask what would consume it and build that instead*** — the loop is what turns a
  design into something that can be wrong.
  **PAGES FILL OLDEST-FIRST AND ARE READ NEWEST-FIRST, and getting it backwards violates §4.3 rule 3
  through the ordinary act of posting.** Rule 1 makes pages key-addressed so *"rewriting page 12
  changes page 12's binding and nothing else"* — which only holds if a new entry lands on the **last**
  page. Fill newest-first and every publish shifts every entry one slot, so the whole archive is
  rewritten and every reader's cursor dies. **Measured, not argued:** the neuter reds
  `posting_again_rewrites_only_the_last_page` with **page 0's content hash moving between two
  publishes**. Within a page the order is the opposite (§4.5, newest-first, *authored* not derived),
  so the two run against each other by design and a test that only checked the round trip would pass
  with **both** reversed — which is why `pages_fill_oldest_first_and_read_newest_first_within_a_page`
  pins the plan directly.
  **`signer = author` IS FEED'S SHORTHAND FOR A FIELD IN SOMEBODY ELSE'S TYPE.** §1.1 says the
  signature carries *"`target = entry_hash` and `signer = author`"*; `system/signature` is the
  **kernel's** type and its `signer` is *"content hash of the signer's identity entity (**NOT**
  peer_id string)"* — the kernel shouts that parenthesis. An implementer reading FEED alone puts a
  peer id in a `bstr` slot that wants a hash and produces something no verifier can use. ***When a
  convention assigns a field it did not define, open the defining document.***
  **A CHECK WHOSE NEUTER ONLY MOVES THE ERROR LABEL IS A REPORTING CHECK — and the module doc said
  otherwise until the neuter ran.** `attribute` cross-checks the signature's `signer` against the
  author's rebuilt identity hash, and the doc claimed that refused a copied signature. It does not:
  deleting it leaves a stranger's signature landing on `BadSignature`, because `verify_for_key_type`
  against the author's key already refuses it. What the check actually buys is §1.1's `signer =
  author` being enforced at all, and *"somebody else signed this"* staying apart from *"these bytes
  are wrong"*. ***Describing a reporting check as a defence is the overclaim a security review then
  has to unpick*** — and the neuter is what tells you which you built. Verification needs **no key
  distribution and no second fetch**: a canonical Ed25519 peer id embeds its own public key, so a
  peer id that does **not** (Ed448, the legacy SHA-256 form) is its own outcome —
  *"we could not check"* must never render as *"this is not theirs"*.
  **`vocab-lint` WENT RED ON A PATH, WHICH IS THE SECOND INSTANCE OF ITS PATH/TAG CONFUSION AND THE
  FIRST IN THE EXPENSIVE DIRECTION.** `spec vocab` skips a would-be tag only when the literal ends in
  `/` — the rule added after its first run reported `app/share/records/` as undeclared vocabulary.
  `app/feed/index` is a **complete** path, so it carries no trailing slash, and the gate reported
  `implemented-undeclared app/feed/index`: a type tag no spec declares and **we do not emit**.
  `_DECL_PARAM`'s own note calls accusing a conformant seat of inventing vocabulary the costly false
  positive, and this is it. Our keys are **derived from a leading-slash literal** now
  (`feed::index_head_key`), which also collapsed two expressions of §4.2's pinned paths into one —
  so the repair is C15's rule and the green gate is a side effect, **not** a fix to the analyzer.
  The next bare `app/feed/…` key literal anywhere trips it again. Routed.
  **§4.3 RULE 6's FALLBACK NAMES A PREFIX THE CONVENTION NEVER DEFINES, and for us it is not a slow
  path but no path.** *"A reader that cannot fetch \[the index] falls back to enumerating the
  prefix — slower, same answer."* FEED pins **two** paths, both index ones, and §2 says outright
  *"the cross-impl contract is the type tag, not the path"* — so the fallback is a type-filtered
  query, and **our foreign-tree consumer resolves a KEY with no type query at all.** `NoIndex` is
  therefore terminal here where the convention says it should be recoverable. Routed rather than
  papered over: synthesising a prefix walk would make us the seat that decided where entries live.
  **THE FIRST CUT OF THE GATE HAND-DECODED THE PROJECTION, WHICH WOULD HAVE MEASURED NOTHING.** It
  walked `{peer}/**.bin`, decoded the pointers itself and resolved the blobs — *a decoder written in
  the same file as the encoder*, which is `make crossimpl-site`'s own lesson (**we consume their
  types, we never model them**) pointed inward. It reads through `SignedSession` now, so the gate
  covers the manifest, the root signature, the trie walk and the two-hop fetch. **And it carries an
  anti-vacuity guard that is the whole difference between the two:** flip one byte of the published
  root and the read must fail — *"the feed read back"* is otherwise true of any rig that reads files
  out of a directory.
  **⚠ `make wasm` WENT RED AND `make test` + `make lint` COULD NOT HAVE SEEN IT — first module in
  this arc where that rule bit.** `feed_publish` imports `RootProjector` and `publish_fixture`, both
  `cfg(not(wasm32))` (a publisher writes a directory; the browser has none), so the WASM build failed
  on two unresolved imports while **1713 native tests and every linter stayed green**. The module is
  `#![cfg(not(target_arch = "wasm32"))]` now. The near-miss is the lesson, not the fix: the gate list
  was about to be written down without `wasm` **on the reasoning that the change was native-only**,
  which is exactly the reasoning *"always run `make wasm` after changes"* exists to refuse — the
  native suite cannot tell you what your imports cost the other target.
  **Stated bounds.** `app/feed/entries/{hex}` is **our** key, not the convention's (§2 leaves it
  local); it is content-addressed so a mutable key cannot move different bytes under a `via: path`
  hint. `collection` and `mirror` are still unbuilt. There is still **no resolver**, so every
  `REFERENCE` requirement phrased *"a reader that resolves…"* remains unmeasured. And nothing here
  is reachable from the product: no window, no verb, no wire.
- **THE FEED SURFACE SHIPPED, AND ADDING A WINDOW FOUND A DEFECT IN THE CENSUS THAT GUARDS WINDOWS
  (2026-09-10).** `src/views/feed/` + `src/dom/feed.rs` — follow a publisher by peer id, read what
  they posted, with `FEED-R4`'s verdict on every entry. The **first product surface** over
  `APP-CONVENTION-FEED` and the first consumer of `feed_read` / `feed_fetch` / `feed_follows`.
  **`tests/window_hydration_census.rs` FLAGGED IT FOR SAYING IT DOES NOT PERSIST WINDOW STATE.** The
  census greps a surface's sources for `window_state_path`; `views/feed` names that path twice, both
  times in module docs explaining that it deliberately keeps clear of it — so the gate demanded a
  hydration classification from a window with no state to classify, which could only be satisfied by
  putting a false row in the table. **AP29 (a gate that counts prose) in its expensive direction.**
  The stripper already existed (`code_only`) and **`persisting_surfaces` was the one caller not
  routed through it** — and `code_only`'s own doc said it *"only has to stop the two false positives
  this census can actually produce"*, naming them. ***An enumeration of the false positives a check
  can produce is a claim about code nobody has written yet.*** Fixed by making every source predicate
  read code, with `a_surface_that_only_mentions_the_path_in_prose_is_not_persisting` as a **two-way**
  falsifier — prose alone is not persistence, a real write still is, and prose *above* real code must
  not mask it.
  **`i18n-locale-check` CAUGHT C15 INSIDE THE CATALOG**, which is a use of that gate worth knowing
  about: a fresh `feed.refresh` key rendered the English word *"Refresh"* differently from the
  existing `btn.refresh` in **four** locales. The fix is not to unify four translations, it is that
  **one English word gets one key** — a second key for a string the catalog already has is drift with
  a translator on the other end of it. 22 keys × 30 locales; append to the locale files, never
  re-serialize them (a `sort_keys=True` reformatted all 30 into a 3,368-line diff before it was
  caught — *verify a scripted edit by reading the diff, not by the script reporting success*).
  ⭐ **SECOND INSTANCE 2026-09-15, BY A SESSION WITH THIS PARAGRAPH IN CONTEXT, AND THE REASON IS
  WORTH MORE THAN THE RULE: the catalogs are NOT sorted, so `sorted()` is not a no-op on them.**
  A script that wrote back `OrderedDict(sorted(...))` produced **4140+/3720−** across the 30 — its
  own per-file report said *"811 → 825 keys (+15)"* for every file, which is **true and says nothing
  about line order**. Reverted and redone as an in-place append: **480+/60−**, sixteen lines per
  file. ⇒ ***a script reporting the right COUNT is not a script making the right DIFF*** — the
  instrument to reach for is `git diff --stat` before `git add`, and the expected number is
  *lines you meant to change × files*, not a key total. The rule above said *append, never
  re-serialize*; what it did not say is that **the files look sorted and are not**, which is exactly
  what makes `sorted()` read as a normalization rather than a rewrite.
  ⭐⭐ **THIRD INSTANCE 2026-09-16, AND IT IS THE HOLE ALL FOUR i18n GATES SHARED: A WORDING CHANGE
  WITH UNCHANGED PLACEHOLDERS WAS CAUGHT BY NOTHING.** Measured, not reasoned — one deliberate edit
  (`"Add connector"` → `"Add rendezvous node"`, no locale touched) and `i18n-locale-check`,
  `i18n-callsite-check`, `i18n-untranslated` and `i18n-lint` were **all four green**. Each is correct
  about its own subject: parity, slots, plurals, script, raw literals. **None of them is about a
  translation still MEANING the English**, so the 30 overlays go stale silently and the app ships a
  locale saying something the product no longer says. ⇒ ***that is strictly worse than a missing key***,
  which `i18n-locale-check` refuses loudly, because **a wrong translation renders perfectly**.
  Closed by `tools/i18n_drift_check.py` (in `make lint`): one row per key carrying a digest of *the
  English the overlays were translated from*, so a moved English value fails asking the only question
  that matters — **did you resweep, or are you taking the debt?** Falsified three ways, each landing on
  its own arm (MOVED · ORPHANED · UNRECORDED), and the ORPHANED arm exists for `vocab-lint`'s reason: a
  baseline outliving its subject re-admits debt, so the comparison must fail **both** ways.
  **Taking the debt is a first-class answer and it PRINTS.** A row marked `stale` keeps the gate quiet
  about that key and names it on every lint run — the `doctor.rs` lesson made structural: *a debt with
  an instrument behind it gets paid; one that lives in a sentence does not.* The regenerate
  (`--write`) **preserves `stale`**, deliberately: otherwise the escape hatch for *"the gate is
  nagging"* would also be the escape hatch for *"the debt is gone"*. Six rows are stale as of
  2026-09-16 — the connector→rendezvous-node vocabulary fix, English only, on the operator's call with
  a release in flight.
  ⚠ **The `git checkout` hazard bit TWICE in the hour this gate was built, in both directions, by a
  session that had the rule in context.** The charter already says *commit the gate before you
  falsify it*. Two additions, and the second is the expensive one.
  **(a) On an UNTRACKED file, `git checkout <file>` is not a revert at all** — it errors, and a
  trailing `|| true` swallows it. The neuter's `sed` on the brand-new baseline stayed applied. *Verify
  a restore by reading the file, not by the command exiting 0.*
  **(b) On a TRACKED file it is a revert of EVERYTHING, and that is what actually cost work:** a
  `git checkout src/i18n.rs` reverting a one-line neuter silently took **six unrelated copy edits**
  made twenty minutes earlier with it. The whole point of the rule is that a neuter is a destructive
  experiment on the working tree — and the tell is not the checkout, it is that **`git status` after
  it does not list a file you know you edited.** ⇒ ***read `git status --short` before `git add`, and
  ask which of your own edits are missing from it***; the six were noticed only because the file was
  absent from a four-line status listing.
  ⭐ **And the recovery is the best evidence the gate works:** re-applying the six made
  `i18n-drift` red with **6 MOVED rows**, against unswept locales, on a real change nobody was
  testing it with. *A gate that catches your own accident is worth more than the neuter you wrote
  for it.*
  **Three modelling decisions worth not re-deriving.** (1) **Who you follow is APP-scoped, not
  window-scoped** (`app_paths::feed_follows_prefix`): two Feed windows must agree and closing one must
  not unfollow anybody, so it is a property of the profile. That is also what keeps this surface clear
  of AP42's reused-slot hazard **by construction rather than by a guard**. (2) The **selection and the
  notice are session-only** — which of your publishers you happen to be looking at is not a fact about
  the profile, and a refusal that survived a reload would be a stale answer to a question nobody asked
  twice. (3) `Peers::seed_remove` is new, mirroring `seed_write` on **the same arm rule**: a surface
  that seeds through one and deletes through `dispatch_remove` is on two different arms for one
  registry, so on Direct a row removed by a button reappears for a frame.
  **The panel has four states and the middle two are the point:** *nobody selected* / *loading* /
  *posted nothing* / *failed*. A walk in flight rendered as an empty feed tells somebody a publisher
  has written nothing, which may be false and which they have no way to question — `FeedStep::Wait`
  one layer down exists to stop exactly that, and this is where it becomes visible.
  **AND THINKING ABOUT THE BROWSER GATE FOUND A DEFECT THE NATIVE GATES COULD NOT — the reader never
  consulted the origins registry.** `OriginFeedSource` was constructed with
  `PinnedPublisher::from_peer_id("", author)`, an **empty origin**, which makes every fetch relative
  to the page: right for a publisher hosted at the app's own origin, and silently wrong for every
  other one, producing a 404 that renders as *"this publisher has no feed"*. Every native gate passed
  because they all point a directory-backed `BinSource` at a temp dir, where a relative URL is
  correct. ***A test double that supplies the thing you forgot to ask for cannot notice that you
  forgot.*** The origin is a parameter now, sourced from `origins::get_origin` — the accessor that
  resolves supersession, so this surface is inside AP54's chokepoint rather than the eighth one
  outside it — and **`FeedPanel::NoRoute` is its own state**: a publisher we have no route to is not
  a publisher whose walk failed, and the two send a person to different places.
  **`make e2e-worker T=the_feed_window` IS THE BROWSER GATE, AND ITS OWN ANTI-VACUITY GUARD WAS
  VACUOUS.** The claim is *a pending question is never rendered as an empty feed*; the rows are the
  body rendering, a fresh profile saying it follows nobody, a refused follow reaching the screen and
  adding no row (**the whole draft → `Action::WindowEvent` → model → re-render loop**, which no native
  test touches), and a followed publisher with no route saying so. **Three neuters, all falsified:**
  collapse `NoRoute` into `NoPosts` → reds printing the production sentence verbatim (*"This publisher
  has not posted anything"* for somebody nobody asked); drop the notice render → reds at the refusal
  row; render nothing at all → reds VACUOUS.
  ***That third neuter is the finding: row 1 originally asserted `text.contains("Feed")`, and the
  window CHROME draws the title — so an empty body passed row 1 and was caught by row 2 instead.***
  A guard that leans on the next assertion is not a guard, and it would have gone silent the day row
  2's sentence changed. It asserts on the **hint**, which `dom::feed::render` draws and nothing else
  does. **When a gate reads a whole window's text, ask which of the words come from the frame.**
  **`CORE_RUST_REF` earned its keep mid-run**, which is its first use for this: the sibling broke
  `bindings/sdk` while a neuter was in flight, and pinning to their last commit made the difference
  between *"my neuter did something strange"* and *"their tree moved"* one flag wide.
  **Stated bounds:** nothing durable of anybody else's is held (no offline read, D24 not engaged);
  and **the WINDOW cannot post** — `publish --ingest-feed` is a CLI path, so authoring still happens
  on disk and never in the app. *(The bound that used to sit here — "the entry list, the seven
  attribution verdicts and a successful walk are still native-only, because the rig registers no
  origin and publishes no feed to one" — is retired by the published-leg gate below.)*
- ⭐⭐ **A SURFACE REACHABLE ONLY BY TYPING AN IDENTIFIER IS NOT REACHABLE — the browse list,
  2026-09-16.** Every mechanism this surface needs had shipped: route planning over two legs,
  per-entry verification, mirrors, paging, the composer. A visitor arriving at a deployment still
  could not read that deployment's own posts, because the only way in was pasting a **45-character
  peer id** they had no way to obtain. The bytes were in the signed root, `entity-deployment.json`
  said nothing about them, and `feed_follows` is written only by the Follow button. ⇒ ***when a
  surface asks for an identifier, ask where a person gets one*** — and if the answer is *"from
  somewhere this profile already knows"*, the list is the feature and the box is the escape hatch.
  The tell is cheap and was in the copy: the window's own hint said *"Follow a publisher by peer id"*,
  which is the surface documenting its escape hatch as the front door.
  ⭐ **The set is the ROUTED set — `origins::list_origins`, and nothing new.** A row with no origin
  can only produce `NoRoute`, so offering it is **AP54 pointed forward: a viewer must not offer what
  it cannot open.** That accessor is also already the supersession chokepoint, so a retired publisher
  is resolved rather than advertised, and the deployment's own publisher is in it for free because
  `adopt_deployment_origin` put it there at boot. **Rows are deliberately NOT probed** —
  `publication_probe` would answer *does this peer publish a feed* at **one signed-root walk per
  row**, i.e. O(list) round trips to decorate a list, and selecting a row already answers the same
  question through `FeedPanel`'s four honest states at the moment somebody asks it.
  ⛔⭐⭐ **THE HOME FALLBACK IS RETIRED — 2026-09-17, on the operator's call — AND THE ARGUMENT THAT
  DEFENDED IT IS THE LESSON. *A DEFAULT VIEW IS DERIVED, NEVER WRITTEN*** said: `effective_selection`
  is *explicit choice, else the home publisher*, computed per render, so it fights nobody, cannot go
  stale, and never becomes a durable record — *"derived and session-only, it is a view, so the
  ownership census never engages."* **Every clause of that is true and it answers the wrong
  question.** Deriving instead of storing settles the **persistence** objection — is this a value
  D25 has to arbitrate — and leaves the **authority** one untouched: *may this surface decide, on a
  visitor's behalf, whose posts they are reading?* ⇒ ***"it is not persisted" is not an answer to
  "it was not yours to choose"*** — and a rendered default is a decision whether or not anything
  writes it down.
  The operator's correction is the premise: **the site's publisher is just another peer.** Knowing
  where they are hosted is the same routing fact that puts their *sites* in reach, and it licenses
  putting them in the list — nothing more. *The feed reader is not the feed publisher for my feed.*
  What it produced on screen is the tell: a panel headed *"This site's publisher"*, their row sorted
  first, their posts loading before anybody asked — **an injected following with a plausible name for
  itself**, which is AP54's invention one step politer. It went through review, shipped with six
  native gates, and the headline gate asserted it **by name** (AP45).
  Gone: the fallback, the home-first sort, `feed.panel.home`, `feed.known.home`, `Selection::home`,
  `KnownRow::home`. The row stays; the ways in are the browse list, the Registry Browser's *Open in
  Feed*, and the peer-id box. The **rest of this entry stands** — the routed set, the no-probing
  rule, the five watches and the anchor lesson are all unaffected, which is why it is corrected here
  rather than replaced.
  **Two more `watch_prefix` calls, both load-bearing rather than tidy** (five now): the origin
  registry is filled by `boot_phase2`'s adoption, which lands **after** a startup window has been
  spawned, and the session config settles there too — so without them the list renders empty on the
  one boot that matters and never re-renders, and on the Worker arm an unsubscribed prefix answers
  EMPTY/`None` from the mirror, which would make the fallback unreachable on the arm that has it
  hardest. *The dead-button defect two registries over, in the place it would read as "this
  deployment knows nobody."*
  ⚠ **AND THE GATE LESSON, which is the one to carry off this repo: AN ANTI-VACUITY ANCHOR MUST NOT
  BE A COPY STRING.** Three Feed gates guarded a vacuous render with
  `text.contains("Follow a publisher by peer id")`, under a comment explaining it was chosen because
  *"`dom::feed::render` draws it and nothing else does"* — true, and it is a **sentence translated in
  thirty locales**. Rewording the hint (which this change had to do, because it no longer described
  the way in) reds three gates about nothing. `READ_FEED` already knew better everywhere else —
  `data-via` carries its own note about the no-route sentence going stale and being red for a day —
  and the vacuity guard was the one place still reading words. It reads `[data-field="feed-peer"]`
  now. ⇒ ***"only the body draws it" is a reason to pick a structural marker, not a reason to pick a
  sentence***; copy is the thing most likely to change for reasons unrelated to the gate.
  **Gates:** six native, headline **`the_deployments_own_publisher_is_offered_and_never_read_on_a
  _visitors_behalf`** — the inversion of the one that used to stand here, asserting **both halves**
  (the row IS offered, so a surface that simply lost the publisher cannot satisfy it, and nobody is
  selected). Make the list the *followed* set instead of the routed one and all six red. Browser:
  the routed-set rule rides `a_gathered_feed_reaches`' step 3, which already establishes a followed
  author with no origin, and the probe emits **`known_ids` rather than a count** — the rule is a
  claim about *which* peers appear and a count cannot falsify it.
  ⛔ **What this does NOT close: DISCOVERY.** The list is *what this profile can already reach*, which
  is a routing fact — how a reader learns a publisher exists at all is still `EXTENSION-DISCOVERY`'s
  question, FEED §9.5's open navigation half, and the reason `publication_probe`'s module doc ends
  where it does. Nothing here invents an answer to it.
- ⭐⭐ **A FIELD ON AN OUTPUT TYPE IS NOT A FIELD ON A SCREEN, AND NO GATE HERE COULD TELL — the
  missing dates, 2026-09-17.** `EntryRow::created_at` was populated from the day the type existed,
  carried correctly through ingest, the trie, the two-hop walk and every one of the seven
  attribution verdicts, and **reached no renderer**. A feed — a *timeline* — rendered body, hash and
  verdict with no date anywhere, for as long as the surface has existed. Reported by the operator,
  not by anything we own.
  ⛔ **The population argument, in a new place: every gate on this surface reads the MODEL.** The
  native tests assert `row.created_at == NOW_MS` and pass, correctly; `READ_FEED` collected
  `entries`, `via`, `attributed`, `body_renders` — every fact the *model* decides — and nothing that
  would notice a field the renderer never asked for. ⇒ ***a renderer-neutral output type makes the
  model testable and makes "is it drawn" unaskable***; when you add a field to one, name what draws
  it in the same commit, or the type is where the fact goes to be correct and invisible.
  ⭐ **The date is rendered VERBATIM, by the platform.** `FEED-R8` forbids rejecting an entry for an
  implausible timestamp, so special-casing an epoch-zero into *"no date"* is that refusal wearing a
  formatter's clothes. `util::local_datetime` is the one expression (`app_saves::stamp_label` was
  the second and delegates to it) — the browser's own locale and time zone, because a hand-rolled
  `YYYY-MM-DD` is the app deciding what a date looks like for someone whose convention it does not
  know.
  ⚠⚠ **AND THE GATE LESSON IS THE FIFTH CAUSE OF A NEUTER THAT PASSES, MEASURED HERE: THE GATE READ
  THE DECISION AND THE DEFECT WAS IN THE EFFECT.** The filter's browser row asserted `data-shown`,
  an attribute `apply_filter` writes from its own count. Neuter the **hide** and leave the **count**
  — one line — and the model's claim is perfect, nothing is hidden, and the gate goes **green**.
  The four causes this file already records are *the gate does not measure it* · *the neuter did not
  land* · *the thing you neutered does not do what you thought* · *the rig cannot produce the
  condition*. This is none of them: the gate is sound about a real property, and that property is
  the surface's **report of what it did** rather than what it did. It is `AP44`'s *witness, not a
  notification* pointed at a test, and `--prune`'s *a value that is only printed is not a guard* one
  layer out. ⇒ ***when a surface both acts and reports, a gate on the report is satisfied by half
  the implementation*** — count the DOM, then assert the report **agrees** with it, so a lying
  counter and a hide that never happens each red on their own.
  Two smaller ones from the same probe. **`known_home`/`known_selected` had never been asserted by
  anything** — collected every run, read by nobody, which is the same defect in a test file; they
  are `known_selected_ids` now, which the new gate falsifies. And **a scripted edit anchored on a
  script preamble landed 12,000 lines away**: `let script = format!(\n r#"\n const layer = …` is not
  unique in a 27k-line test file. The charter's rule is *anchor on a row id unique in the whole
  file*; the addition is that **in a test file the unique anchor is a function SIGNATURE**, because
  script preambles are copied by the dozen and every one of them is a plausible match. Caught by
  grepping for the marker afterwards; restored by inverse edit, not `git checkout`.
- ⭐⭐ **A TAB IS *INSTEAD*; A DISCLOSURE IS *EXTRA* — and no ORDERING of a stack fixes a window
  whose first pane holds an archive (2026-09-17, `components::tabs`).** The Feed window's three
  surfaces — read somebody, write your own, administer the list — were stacked, and the two fixes
  before this one both stayed inside the stack: put reading first (09-16), then collapse the other
  two behind headers (09-17). Both were right about the *diagnosis* and reached for the wrong atom.
  With a real feed on screen — **34 posts, and that is a small archive** — *Your feed* sat a screen
  and a half below the fold, so posting something of your own meant scrolling past everything
  somebody else had written. ⇒ ***whichever pane is second is under the first one's content, so if
  the panes are alternatives the fix is not an order and not a collapse.*** The tell is in the
  vocabulary: reach for `collapsible_header` when a section is **extra** (a create form, fields most
  people never touch) and for `tabs` when it is **instead**.
  ⭐ **The reading pane is TWO PAGES and the SELECTION decides which** — the list of publishers, or
  one publisher's posts, with a Back control. That is the Knowledge Base's list/reader shape and the
  Site Browser's navigation, both of which already existed here and neither of which this window
  used: *"the site browser had it figured out, you navigate like a website."* **No second bit saying
  which page** — a bit that can disagree with the selection eventually does, and *back* is exactly
  *nobody is selected*.
  ⚠⚠ **AND THE GATE LESSON IS THE ONE TO CARRY: A CONSISTENCY CHECK BETWEEN TWO PROBE FIELDS IS NOT
  AN ANTI-VACUITY GUARD — BOTH FIELDS GO TO ZERO TOGETHER.** `READ_FEED` reads the whole window, so
  every field in it is now scoped to **whichever pane is open**, and the gates that asserted facts
  from two panes in one read had to start navigating like a person. The one that would have gone
  quietly wrong is the gathered gate's routed-set rule: a **negative** (*this unreachable author must
  not be offered*) guarded by `known_ids.len() == known` — which is `0 == 0` the moment the browse
  list is on a pane nobody opened. It is a **floor** now (`!known_ids.is_empty()`, true because that
  deployment's document registers an origin) and the read moved to before the follow, where the list
  is actually drawn. ⇒ **when a surface becomes navigable, re-read every probe field for what it now
  answers when the thing it counts is simply not on screen** — and prefer a floor a scenario
  guarantees over an equality between two things the same defect silences.
  Three smaller ones. **Follow is the one act that changes the selection without navigating** —
  it is reachable from all three panes, and somebody working through a list of peer ids in the box
  would be thrown out of it on the first press; *back* likewise does **not** forget the walk, which
  is the opposite of `unfollow`'s rule and the difference between *show me the list again* and *stop
  wanting them*. **A control that cannot help raises the question of what it is for**: the filter box
  now appears only above `dom::feed::FILTER_FROM` posts, and the pair either side of it is gated (3
  posts → none, 34 → one), so the line is a measurement rather than a taste. And **two `///` blocks
  with a function between them fuse onto the second one** — the composer's doc had been attached to
  `place_body` for as long as both existed, invisible to every gate here, because prose is not
  checked and a slid doc block reads perfectly in the source.
- **THE ORDINARY LEG HAD NO BROWSER GATE AND THE EXTRAORDINARY ONE DID — closed 2026-09-12,
  `make e2e-worker T=a_published_feed_reaches`.** A §6 mirror carrying an author a browser cannot
  reach had a gate; *follow a publisher and read what they published at their own origin* did not,
  **because no rig here had ever published a feed to an origin it also served.** So `feed_read`'s
  two-hop walk, `SignedSession`'s root check, `FEED-R2`'s detached signatures and `FEED-R4`'s
  verdicts all ran in WASM for the first time when this gate was written — and every one of them
  went green first try, which is the part that makes the gap easy to keep. ⇒ **when you gate the
  hard case first, write down that the easy one is ungated**; the hard case's gate reads as coverage
  of the subsystem, and a rig built for the exotic scenario is usually *unable* to produce the
  ordinary one. Here the mirror rig's whole design is *the author is served nowhere*.
  ⭐ **EVERY FEED FIXTURE ON EITHER SEAT WAS ONE PAGE, SO `FEED-R12` WAS UNFALSIFIABLE EVERYWHERE.**
  The fixture publishes **34** entries against `DEFAULT_PAGE_SIZE` 32 and `feed_fetch::LIMIT` 50 —
  deliberately across a page boundary and under the limit — so the oldest post lives on the index
  page the head does **not** name. Falsified: stop the walk after the head's page and it reds
  showing **2 of 34**. A one-page population cannot tell a reader that walks from one that got
  lucky, and *"both ends of the archive"* means nothing when both ends are on one page.
  **The oversized post is a row, not padding:** one body is over EMBED §3's 16 KiB inline ceiling,
  so the archive a browser walks contains an entry on the **pointer** arm — gated natively at the
  chunker and at the publish closure, never before at a consumer. (What it proves is that the entry
  *resolves*; this surface renders no bodies, so the blob is still only gated by `--verify`.)
  **Three neuters, three distinct rows:** drop the published leg from `feed_route::plan` → the
  no-route sentence verbatim; never consult the per-entry signature → entries still arrive and
  `attributed` goes 34 → 0; stop after the first page → 2 of 34.
  ⭐ **MEASURED, AND IT BEARS ON §6.2's COST ARGUMENT: reading ONE author's 34-entry feed cost 71
  `no-store` fetches of that publisher's `system/peer/published-root`** — 2 per entry (body, then
  signature) + 1 per index page — counted off the origin's access log. **Not a defect:**
  `SignedSession::resolve` re-fetches the manifest on every resolve *by design*, and says why
  (*"a stale manifest can never roll back, because we never look at it twice"*) — the anti-rollback
  `seq` floor is bought by that refresh. What it corrects is a reading of §6.2: the gatherer's
  *"one root check instead of 500"* is about the number of **distinct publishers** to verify, and a
  mirror walk pays the same per-resolve refresh against one root. **Budget ~2N round trips to the
  root on top of the bodies**, and do not read §6.2 as promising otherwise. Narrowing the refresh to
  once per *walk* would preserve the floor and is a change to an anti-rollback property — its own
  session, its own gates, not a line in a feed change.
- ⭐ **A REPORT-ONLY COMPUTATION IS STILL ON THE CRITICAL PATH — the whole publish path's scaling
  ceiling was a diagnostic nobody reads (2026-09-13).** `RootProjector::finish` tested closure
  membership with `self.bindings.values().any(…)` **inside** the loop over the trie closure —
  `O(|closure| × |bindings|)` — to compute `trie_nodes`, a number that is printed and used by
  nothing. Measured through the real projector at 16,000 posts: `finish` **20.37 s → 1.76 s**,
  quadratic to linear, and **nothing else in the emit path was superlinear** — so for as long as
  that line existed, the ceiling on *every* publish here (sites, apps, the registry, the feed) was
  one counter. ⇒ **when a publish feels slow, profile the reporting before the work.**
  ⭐⭐ **THE REASON IT SURVIVED IS THE TRANSFERABLE HALF: WHEN A DESIGN ARGUES A COST PROPERTY,
  MEASURE THE COST.** `APP-CONVENTION-FEED` §4.3 rule 1 is a `[MUST]` *about a feed at scale* —
  *"rewriting page 12 changes page 12's binding and nothing else, `O(tree depth)`, the same cost as
  posting"* — and **every fixture on either seat was 3–34 entries.** The property itself holds
  exactly (`the_cost_of_posting_one_more`: at n=4000, posting one more moves **3 bindings out of
  8,129**) and the publisher **re-emits all 8,129**, 17,275 files, every post. *Rule 1 is about a
  reader's invalidation surface and we get it for free; nothing asks the publisher to exploit it and
  we do not.* Both probes are `#[ignore]`d in `feed_publish` and assert nothing —
  `make test-one T="the_cost_of_a_large_feed --ignored"` — because a threshold nobody has earned is
  a flake and the point is the shape of the curve. ⇒ **a conformance fixture is not a scale
  fixture**, and a convention whose whole argument is cost needs one of each.
  Full picture, with the read side, the unbounded mirror and the live leg:
  `docs/plans/AUDIT-2026-09-13-THE-SOCIAL-TIER-AT-THE-CHECKPOINT.md`.
- ⛔ **`git checkout <file>` IS THE NEUTER-REVERT EVERYONE REACHES FOR, AND IT DISCARDS EVERY
  UNCOMMITTED CHANGE IN THAT FILE — including the gate you just wrote (2026-09-13).** Falsifying a
  gate means editing the product, running, and reverting. Reverting with `git checkout` is right
  exactly when the file holds nothing else of yours; the third use of it in one session was on
  `publish.rs`, which also held **two brand-new tests**, and it took both. ⇒ **commit the gate
  BEFORE you falsify it.** A neuter is a destructive experiment on the working tree, so the thing
  being measured should already be in history — that also settles *which* green you are reporting.
  Otherwise revert with the inverse edit, or `git stash push -- <file>`, never the file-scoped
  checkout.
  ⭐ **What caught it is the direction of *a count is not a claim* nobody uses: a count that did NOT
  move when you added something is itself a claim.** `make test` came back with the main binary at
  **1754 passed / 19 ignored** — byte-identical to the figure before either test existed — after a
  run that should have read 1755/20. Both gates had genuinely passed before the loss, so every green
  reported was real when measured and the tree no longer contained what produced them. **Read the
  delta, not the colour**, and know what your own change should do to it.
- **AN AXIS WITH A FLAG AND NO STAGED `make` VARIABLE IS AN AXIS A PERSON CANNOT PUBLISH — `FEED=`,
  2026-09-12.** `--ingest-feed` shipped 2026-09-10 with a row in the flag table and **no make
  plumbing at all**. Every containerized publish target bind-mounts only this repo, and
  `stage_publish_sources` is what copies an out-of-mount source in and rewrites the path — so a bare
  `--ingest-feed=` at a `make` invocation names a directory the container cannot see, and on a
  podman-only host (which is the supported host) the third axis was reachable **by nobody** for two
  days. `FEED=` is staged exactly like `INGEST=`/`APPS_DIST=`, wired at **all three** call sites of
  the staged-flag pair rather than the one in front of me (AP44 — and the pair is greppable, which is
  what made the enumeration cheap). ⇒ **when you add an ingest flag, the deliverable is the flag, the
  staged variable, the `.gitignore` row and a worked example** — the canonical home for which is
  `REFERENCE-PUBLISHING-PIPELINE` §0.2's axis table, which now carries a `make` column and an example
  column for exactly this reason.
  **The worked example is `examples/entity-demo/feed/` and the point of it is coexistence** — one
  authoring root carrying two sites *and* a feed, published in one run under one signed root, which
  is the shape the browser gate above then reads back. Two application-tier conventions, one
  identity, one wire.
  ⛔⭐⭐ **THIRD INSTANCE, 2026-09-17 — AND IT IS NOT THE LESSON FAILING TO FIRE. THE EXEMPTION WAS
  WRITTEN INTO THE SAME TABLE AS THE RULE THAT REFUTES IT.** `--gather` shipped 2026-09-12 with a
  flag, a spec section, four gates, and a `make`-column cell reading *"— (no make variable: the value
  is a `peer@dir` pair, and the dir is another publisher's out-tree rather than an authored source)"*
  — i.e. the author **did** consult the deliverable list and wrote down a reason. **Three lines below
  that cell, in the same section, is the sentence that answers it:** *"a source anywhere else on the
  host is invisible to the publish … a raw `--ingest-feed=` therefore names a directory the container
  cannot see."* Nothing about *whose* tree it is or *what shape the argument has* touches that; the
  constraint is the bind mount. So the fourth axis was reachable by nobody on the supported host for
  five days, **behind a stated exception rather than behind an omission** — which is strictly harder
  to find, because the cell reads as considered. ⇒ ***an exemption's reason has to answer the rule's
  reason, not the rule's subject*** — *"it is not an authored source"* is a fact about the argument
  and the rule is about the container. Same shape as `A-30b` (*we stated a rule and proposed breaking
  it in the same packet*), arriving in a reference table instead of a packet. **The cheap check: when
  you write "N/A" in a column, re-read the paragraph that says why the column exists.**
  `GATHER='<peer>@<dir> …'` is staged like its three siblings now, each source under the **author's**
  peer id rather than an ordinal (`parse_gathers` already refuses one author twice, so the id is
  unique by the rule that matters). **Run before it was written down** (AP37): 34 entries, 34
  attributable, two pages, two cuts byte-identical but for the wall-clocked published-root.
  ⇒ **the enforcement point this earns is the axis table itself** — `PublishAxis` is a closed list in
  one file, so *"every axis has a `make` variable"* is a census somebody can write; it is not written
  yet and that is the honest state.
  ⚠ **AP37, found on the way: `examples/demo-site/` does not exist and never has.** **Five** places
  named it across three documents — the Makefile's `INGEST=` row and its `site-serve` example,
  `PUBLISH-INGEST-FORMAT`'s KEPT list, and two rows of `TOOLS.md` — while the directory is
  `examples/entity-demo/`. *A documented invocation is a coupling no compiler maintains*, and the
  tell was cheap: `ls` the path in the doc you are about to copy a line from. **The first count
  written here was three, from the grep I happened to run first** (`Makefile` + one guide); the
  other two surfaced only from a tree-wide sweep. *A count is not a claim* — sweep before you
  publish the number.
- **A MIRROR IS REACHABLE BY A PERSON — the Feed window's *Read through* list, 2026-09-12. The
  §6 arc's last gap on this seat, and the surface half is `src/feed_gatherers.rs` + the
  `Leg::Mirror` wiring in `feed_fetch`.**
  ⭐ **THE LIST IS TYPED BY A PERSON, AND THAT IS THE DESIGN RATHER THAN A SHORTFALL.**
  `feed_route::plan` has taken a gatherer list since it shipped and been handed an **empty slice**
  by the product the entire time, because §6 gives a reader **no way to learn a gatherer exists** —
  no advertisement, no registry binding, no deployment field, and §6.0.1 is a *derivation* (where a
  mirror would be **if** you knew whose) rather than a discovery. The alternative to a typed list is
  the failure this file already names twice: *a viewer that cannot be told its source invents one,
  and the invention is always "the first thing I already hold"* (`views/games::app_source`, AP54).
  **A mirror makes that worse than usual** — the wrong guess is not a stale publisher, it is **a
  stranger's reading of somebody else** in the place that person's own posts would be.
  ⭐ **A LOSSY PROJECTION OF AN ADDRESS IS THE FIRST THING THAT BREAKS WHEN A THIRD KIND ARRIVES.**
  `Resolution::Served` carried `via: &'static str` — a leg's *name*. The mirror leg has a fact the
  name cannot hold (**whose** reading this is), and §6.1 rule 3 makes the *via* line the **one**
  place a gatherer may be named, so *"mirrored"* without *"by whom"* leaves a reader unable to judge
  a view they have just been told is partial. It carries the whole `Leg` now. The other half is
  structural: **`EntryRow` has no field a gatherer's id could travel in**, so *never a byline* is a
  property of the types rather than a rule a renderer remembers.
  ⭐ **A ROUTE CHANGE INVALIDATES EVERY HELD WALK, NOT ONE AUTHOR'S.** The route is planned per
  render from the live gatherer list, so adding one changes it for **every** author — and a held
  `FeedState` is an answer to the old route that `poll` serves without re-walking. Somebody who adds
  a gatherer *because* an author would not load would otherwise press the button, see the identical
  panel, and conclude nothing happened. `FeedPoller::forget_all`, and its own gate.
  **An unrouted gatherer contributes NO LEG and still SHOWS.** No origin means no URL, and
  manufacturing one relative to the page is `OriginFeedSource`'s empty-origin defect, whose 404 read
  as *"this publisher has no feed"* — *a dropped leg is honest; an invented one is a wrong answer.*
  But dropping the **row** as well leaves somebody unable to tell whether adding it did anything, so
  it stays and says so, and `routed` is recomputed per render rather than remembered (AP41).
  **Two registries, two watches** — `feed-follows/` and `feed-gatherers/` are one path segment apart
  and both keyed by a peer id. Forgetting the second `watch_prefix` is a dead button, and the browser
  gate reads the two lists through **separate selectors** so *"the right list grew"* is falsifiable.
  ⇒ **`make e2e-worker T=a_gathered_feed_reaches_a_browser` CLOSES THE DELIVERY HALF (2026-09-12) —
  and the gate written to prove it works found that it did not.** `FeedWindow`'s factory built its
  poller with an **empty `RepaintCell` that nothing ever filled**, so a landed walk told nobody:
  every byte arrived, every signature verified, and the panel sat on *"Reading this feed…"* until
  some unrelated write marked the window dirty. `render_dom` now composes `dirty.mark()` +
  `repaint()` the way `content_site` does, and for its stated reason — a frame rebuilds only DIRTY
  windows and a feed walk writes nothing to the tree, so a bare repaint is not enough either.
  ⭐ **Two months green said nothing about it, because every earlier gate had a click of its own
  coming**: the sibling browser gate's last step is a press and each native gate drives the poller
  by hand. *A surface that only ever gets looked at right after you touched it never has to tell you
  anything.* ⇒ when a surface completes work asynchronously, ask what LOOKS at it afterwards.
  **The rig's shape is the transferable half.** The author is published to a throwaway directory and
  **served nowhere** — the scenario and the anti-vacuity guard at once, since A has no `origins`
  entry *and* A's carried bytes at B's origin are outside every signed root there by construction
  (`RootProjector::record` skips a foreign peer). And **step 3 asserts the unreachable state BEFORE
  any gatherer exists**: the claim is a *transition*, and a transition needs both of its states
  measured, or a browser that found the posts by some other path passes every row with `Leg::Mirror`
  never consulted. `data-via` / `data-attributed` are read as **attributes, never sentences** — the
  no-route needle one gate over was a copy string and was red for a day.
  Three neuters, each falsified: drop the mirror legs → the no-route screen; drop the repaint wiring
  → the production symptom verbatim; publish A at the same origin under A's own signed root → **the
  CONTROL reds**, reporting the published leg served it.
  **Stated bounds:** nothing durable of a gatherer's is held (D24 not engaged), and no *window*
  publishes a mirror — `--gather` is a CLI verb.
- ⭐⭐ **THE COMPOSER SHIPPED — AND *PUBLISH* IS A WORD WITH TWO MEANINGS, ONE OF WHICH HAS A VERB AND
  A PIPELINE AND A DIRECTORY. THE ONE WITH MACHINERY LOOKS LIKE THE REAL ONE (2026-09-15,
  `src/feed_compose.rs`).** Four verbs — add/remove an entry, add/remove from a collection — writing
  into the bound peer's own tree, plus `app/feed/collection`, the seventh of seven FEED types and the
  only one no seat had built.
  **Writing `app/feed/entry` into your own tree IS publishing.** A connected peer asks and you serve
  your tree, like everything else in it. The **static** road — projecting a signed root and putting
  bytes at an origin — is an *additional*, operational act that freezes a tree into a version others
  can pull while you are down; it already exists (`publish_axes`/`feed_publish`) and mints over
  whatever is in the tree when it runs. ⚠ **This was got backwards twice in one week, once by arch
  (recorded against themselves in their `ROUTING-2026-09-15-d` §1) and once here**, both times by
  treating the static emit as *publishing* and the tree write as a precursor. ⇒ **when a word in this
  system has a verb, a flag and an out-dir behind one of its meanings, check which meaning the
  sentence needs before you reach for the machinery.**
  ⇒ **CANONICAL HOME AS OF 2026-09-16: `REFERENCE-PUBLISHING-PIPELINE` §0.0**, with the two roads as
  a table and *what is built on each*, measured. It went there rather than being restated here for
  AP56's corollary — **the fix for *"the model was not where I looked"* is almost never *"state it
  again where I looked"***, and §0.1 next door was already the canonical statement of the half that
  had one. It is a third instance of the same confusion that put it there: the operator named it as a
  thing this seat keeps re-deriving, and the doc that *is* the model was silent on it while §0's own
  opening line — *"there is no application server"* — is true of the static road and reads as a claim
  about the system.
  ⭐ **MEASURED, AND IT IS A LOAD-BEARING FACT ABOUT THIS PRODUCT: A BROWSER PEER IS NOT A PUBLISHER.**
  Arch ruled the composer as *mint-after-append* on `entity-workbench-go`'s measurement (a 4th entry
  authored with no re-mint; a verifying live reader sees 3, with no error and no warning). Right about
  a peer that mints. **Ours does not.** Through `Peers::new_direct_with_connector` — the construction
  `PeerManager` gives the app — `published_root_head` and `system/tree/root` are **`None` before and
  after a feed write**: `PeerManager` never calls `with_published_root` and `EntitySDKBuilder` has **no
  passthrough for it**, so no prefix is tracked and there is nothing to re-sign. Armed by hand
  (`PeerBuilder::with_published_root("/")`) it works and is cheap — **200 feed puts → `seq` 200,
  exactly one mint per write, 176 µs/put** — and *every* tree write mints, including a window-state
  persist. **Do not describe a browser profile as a publisher without arming that, and do not arm it
  without deciding whether every profile should pay a mint per write.**
  ⭐⭐ **AND THE PREFIX QUESTION IS EVIDENCE FOR THE UNRULED `A-38`:** a root scoped to `app/feed/` —
  arch's own wording — commits every entry and **no entry's signature**, because `FEED-R2`'s detached
  signature lives at V7 §3.5's invariant pointer `system/signature/{hex}`, **outside the convention's
  own prefix**. `"/"` is the only prefix under which a feed is self-contained. *The convention's `R2`
  obligation is discharged at a path the convention does not own.*
  ⛔ **THE SIGNATURE IS THE HALF THAT GETS FORGOTTEN, IN BOTH DIRECTIONS.** An add that writes only
  the entry publishes something `feed_read::attribute` answers `NoSignature` for; a remove that
  unbinds only the entry leaves a signature bound to bytes that are gone, which is exactly the trace
  §7.3 says does not exist (*"byte-identical to a tree that never contained it. Not similar —
  identical."*). Both gated, both neuters falsified **separately**, so the two directions are
  independently measured.
  ⭐⭐ **AN ADDRESS MUST BE CARRIED, NEVER RECOMPUTED — and the obvious repair is silent and total.**
  The authoring surface needs each post's hash, and `feed_tree::read_owned_feed` (the *publish
  projection's* reader) drops it. The reflex is `entry.to_entity().content_hash`. **V7 §2.6 makes
  unknown fields MUST-ignore**, so an entry carrying a term this build does not model — authored by a
  later version of us, another impl, or a convention revision — decodes fine, drops the term, and
  **re-encodes to a different hash**. Remove then builds a key naming no binding, unbinds nothing, and
  prints §7.5's unpublication sentence: *the post stays up and the person is told it came down.* So
  `feed_compose::own_posts` reads the address from the **key**, which is where `entry_key` put it.
  ⇒ **whenever a decode is lossy by design, a value derived from a re-encode is a different value.**
  ⭐ **ALLOWING DUPLICATES MAKES REMOVAL POSITIONAL, and a length assertion sails straight through the
  bug.** §5's *membership is not exclusive* holds **within** one collection — a playlist that plays a
  track twice is an ordinary playlist — so `plan_add_to_collection` does not dedupe. Therefore *"remove
  this member"* does not name one binding: with `[a, b, a]`, a by-value remove takes the first match
  and leaves `[b, a]` instead of `[a, b]`. **Same length, different content, and order IS content
  (`FEED-R15`)** — data loss wearing a no-op's clothes. `plan_remove_from_collection` takes a
  **position**, which makes the ambiguity inexpressible rather than resolved silently (`MirrorSubject`'s
  argument-list guard, one type over) and is the only form a list UI can supply honestly: what the
  person clicked is a row, and a row is a position.
  **`FEED-R21` lives on the button.** §7.5 makes presenting removal as deletion a **MUST NOT** and puts
  the honest sentence *"at the moment of the action rather than in a help page"*, so the removal verb
  **returns** `RemovalMeaning` — a caller cannot take the plan without being handed what it owes — and
  the gate asserts the **rendered string** (no *"delet"*, and §7.5's load-bearing second clause
  present), not the enum variant, because the MUST NOT is about what a person is told.
  **Two rig lessons, both paid for twice.** (1) A `Peers` generates its own primary and has **no writer
  and no SDK for any other peer id**, so a test signing with a stand-in key lands on `Refused` and
  reads as a product defect — `FEED-R1`'s fact one layer down. Both new test helpers had to be
  corrected to author *as the profile's own peer*, independently, in two files. (2) **`tests/escape_hatch_budget.rs`
  refused the obvious way to reach a peer's own identity** (`direct_peer_shared`, an L0 hatch that
  answers `None` on the Worker arm) — **not allowlisted**: the test builds both handles from one seed
  instead. *A budget that can be waived by adding a line is only worth having if you first ask whether
  you needed the hatch at all.*
  **Found on the way and fixed:** `KeyType::label()` is the kernel's canonical V7 §3.5 name and we
  restated it **twice** — `signed_root::algorithm()` through a wildcard (`_ => "ed25519"`, wrong for
  `ExperimentalTest`) and `feed_read::key_type_name` exhaustively. The second's doc said it *"mirrors
  `RootProjector::algorithm` rather than re-deriving"*, and it did not. *A comment claiming a factoring
  is not the factoring* — the tell is the one this file already gives: ask which module would have to
  change if the shared thing changed.
  **Stated bounds:** collections are **model-complete and unwired** (verbs 3 and 4 are gated; nothing
  on screen calls them) · the window still cannot author a collection · nothing here mints, by design.
  *(The "no browser gate for the composer" bound is retired 2026-09-16 —
  `make e2e-worker T=a_default_profile_can_post` — and the entry below is what the concession cost
  while it stood.)*
- ⭐⭐ **THE COMPOSER WAS UNREACHABLE FOR EVERY VISITOR FOR A DAY, BECAUSE THE PROFILE'S OWN KEY IS IN
  THE OTHER DRAWER (2026-09-16).** `feed_compose::authoring_keypair` enumerated the `entity_peers`
  **vault** — which is the *spawn list*, the peers a person explicitly created. Every window on a
  default profile is bound to the **system peer**, whose seed lives under `entity_system_seed`, a
  drawer whose own constant is documented in as many words as *"distinct from the `entity_peers`
  spawn-list"*. So the lookup answered `None` for the one identity the app is definitionally running
  as, and the surface said *"this profile does not hold its key"* about a key in localStorage, held in
  memory, and signing that session's traffic. **Writing into your own tree IS the publish, and the
  surface built to do it refused on the shipped default.** Fixed via
  `persistence::held_keypairs` — the single answer to *may I author as this peer*, knowing both
  drawers, so a third durable identity added tomorrow is one row rather than a sweep of every surface
  that authors.
  ⭐ **THE TRANSFERABLE HALF IS WHY NINE NATIVE GATES AND TWO BROWSER GATES STAYED GREEN, AND IT
  INDICTS A REFACTOR THIS FILE WOULD OTHERWISE PRAISE.** The model splits `post` into
  `post_signed_by(.., signer: Option<&Keypair>)`, and its own doc comment states the reason: *"the
  lookup reads process-global persistence, so a model that did it inline had exactly one reachable
  outcome in a native test (`NotOurPeer`) … a parameter is the difference between a wired surface and
  a gated one."* **That reasoning is correct and the split is good design — and it moved the ONLY
  untested thing (*which drawers do we look in*) outside everything that tests the composer.** Every
  native gate hands the keypair in. ⇒ ***when you split an impure lookup out to make a decision
  testable, the lookup is now the untested half — and it is the half that fails in production.***
  Name it in the doc comment and gate it from a surface, or the split has traded a reachable bug for
  an unreachable one.
  **The population argument at its plainest, and note what the missing shape was.** Not an encoding,
  not another implementation, not a foreign publisher: **a profile nobody had hand-fed a key to** —
  which is to say the only kind that exists in production. *A test double that supplies the thing you
  forgot to ask for cannot notice that you forgot*, third instance (`OriginFeedSource`'s empty origin,
  `feed_fetch`'s two-legged mirror double, this).
  **The fix's shape is a WITNESS, and a read-only re-read would NOT have been equivalent (AP44).** The
  system seed is recorded inside `system_seed()` — the function that mints it — rather than by a
  `remember_system_seed(seed)` added to `new_wasm` beside the existing call, so there is no second
  call site for the next author who constructs a primary to know about. A read-only localStorage
  re-read is a *different answer*: when storage is unavailable `system_seed` returns a fresh
  per-session identity it never persists, so re-reading would report that we do not hold the key for
  the peer we are at that moment running as. `EntityApp::webrtc_seed` already carries that exact
  reasoning for the carrier identity, one consumer over — **the argument was in the tree and the
  second consumer did not inherit it.**
  **Gate: `make e2e-worker T=a_default_profile_can_post`** — the composer is offered · a post lands
  and is on screen (the whole draft → `WindowEvent` → `plan_add_entry` → `apply` → tree → re-read
  loop) · **it survives a reload**, which is what separates *we wrote to the tree* from *we appended
  to a session vector* and is the claim the operator actually made · removal unbinds it without
  calling itself deletion (§7.5). Falsified: restore the vault-only lookup and row 2 reds with the
  production sentence, `can_author=false compose_blocked=true`. **`can_author` and `compose_blocked`
  are read as two separate selectors, never one negated boolean** — a `render_composer` that drew
  *neither* would otherwise pass as the healthy arm.
  ⚠ **And the durability row waits on the STORE, not the render.** The Direct/IDB arm is write-behind
  (250 ms debounce) and a `goto` abandons the pending drain, so a reload issued on the click races the
  flush and the row fails wearing the composer's costume — reporting *"the post was never in the
  tree"* about a post that was. It polls `durable_hash_for("app/feed/entries/")` and **asserts the
  probe found something**, because a probe that silently found nothing is a wait for nothing.
- ⭐⭐ **A SECOND TAB WAS TOLD ITS OWN PROFILE DID NOT HOLD ITS OWN KEY — the same function, the same
  gap, one day later (2026-09-17).** The entry above closed the *default profile* case and shipped
  with `a_default_profile_can_post` green. The composer still refused for anyone with **two tabs open
  on one origin**, which is how it was reported: *"we're back to getting this."*
  **The mechanism is one line and it is not in the composer.** Two tabs at one origin contend for one
  profile, and the Web-Lock election (`src/multitab.rs`) makes the loser **ephemeral on purpose** — it
  must not open the shared IndexedDB database. That arm built its primary with `Peers::new_direct()`,
  whose keypair is generated **inside the SDK constructor and returned to nobody**, so the tab ran as
  a real, signing peer whose key was in neither durable drawer. `held_keypairs` answered *correctly*
  that we do not hold it. ⇒ ***an identity generated where the caller cannot see it is an identity
  nobody holds*** — and every *"do we hold this"* question then answers **no** about the thing the
  process is definitionally running as. `EntityApp::ephemeral_primary` mints it in our own code and
  records it — a witness at the mint, so a third ephemeral construction path cannot forget it.
  **Recorded, never persisted:** an ephemeral arm writing an identity is the vault clobber the 1a
  durability gate exists to refuse. (**The slot holds the KEY.** It held only the derived id for a
  day, on *"a slot holding a signing key is a slot somebody eventually signs with"* — true, and
  protecting against the wrong thing: this session **is** that peer and holds the key in the SDK
  regardless, so id-only bought no safety and cost the one thing the identity is for.)
  ⭐⭐ **THE SPLIT THAT MADE THE DECISION TESTABLE HAS NOW EATEN THE SAME FUNCTION TWICE, SO IT IS THE
  ENTRY AND NOT A NOTE.** The 09-16 lesson — *when you split an impure lookup out to make a decision
  testable, the lookup is now the untested half, and it is the half that fails in production* — was
  written **about this function**, in its own doc comment, and the next defect landed in exactly that
  gap. A pure `decide_author_key` is gated every way; *which drawers do we look in* was covered by
  nothing, twice. ⇒ **the split is still right, and it owes a test of the LOOKUP** — here
  `set_ephemeral_identity_for_test` exists solely so the `SessionOnly` arm, whose only real filler is
  `cfg(wasm32)`, is reachable from a native gate at all.
  ⛔⭐⭐ **THE SENTENCE WAS WRONG *AND SO WAS THE REFUSAL* — corrected 2026-09-17, one day later, and
  the corrected claim is the entry. *PERSISTENCE IS NOT PERMISSION.*** What stood here said the
  refusal was right: *a temporary identity genuinely may not author, the post would land in a tree
  that evaporates, signed by a peer nobody can reach again.* The operator rejected it on sight and
  reading the convention says they were right. **Both halves are false.** It **is** the profile's —
  this session is that peer, holds its key and signs every message with it — and *kept* is a
  different question from *allowed*: nothing in `APP-CONVENTION-FEED` conditions authorship on
  durability, a peer writes entries into its own tree, and that write **is** the publish
  (`REFERENCE-PUBLISHING-PIPELINE` §0.0, which this file already carries two entries about).
  ⇒ ***a surface that withholds a control because the result is temporary has decided, on somebody's
  behalf, that a temporary thing is not worth doing.*** Sibling to the browse-list correction three
  entries down — *"it is not persisted" is not an answer to "it was not yours to choose"* — pointed
  the other way: there a durability argument excused a decision we had made for someone, here it
  removed a decision that was theirs to make.
  **What survives is the AP40 half, and it is the smaller one.** *That peer is not yours* and *this
  tab is running on a temporary identity* really are two facts with different remedies, and
  `can_author: bool` → `AuthorKey` was the right move; what was wrong was putting both under
  *withheld*. Now: **two arms author**, `SessionOnly` carries a **caveat beside a working composer**,
  and exactly one arm refuses — for a reason about *whose peer it is* rather than about how long
  anything lasts.
  ⚠⚠ **AND THE GATE WRITTEN TO FIX THE WORDING PINNED THE REFUSAL BY NAME — AP45, in the place it is
  hardest to see.** `a_second_tab_is_not_told_its_own_profile_lost_its_key` asserted, in its own doc
  comment, *"a temporary identity genuinely may not author… this gate does NOT assert that tab 2 can
  post"*, and the native gate said *"a refusal must not offer the composer"*. Both green, both
  reading as considered decisions. ⇒ ***when you split a collapsed refusal into two, ask of each arm
  whether it should have been a refusal at all*** — the split makes the arms legible and is exactly
  what makes the surviving one look examined.
  ⚠⚠ **THE FIFTH CAUSE OF A NEUTER THAT PASSES, MEASURED HERE: THE PREDICATE HAD NO CONSUMER.**
  Falsifying the browser gate by restoring `AuthorKey::may_author` to `Held`-only came back **GREEN**.
  Not the gate, not the neuter, not the rig: `render_composer` branched on *"is there a refusal
  key"*, so `can_author()` was a predicate **the surface never read**, and the model and the DOM were
  two expressions of one rule (C15) that could disagree with nothing to say so. The renderer asks
  `can_author()` for **where** the note goes and `compose_note()` for **what** it says — two facts,
  all four combinations expressible, only the model choosing. ⇒ ***when a neuter on a decision
  passes, grep for the decision's consumers before doubting the gate*** — a model predicate a
  renderer re-derives is a predicate the product is not using. Re-falsified after wiring, and the
  dump is the whole argument: `can_author: false` under a rendered sentence reading *"You can post,
  and it is yours and signed."*
  ⚠ **The population's missing shape was A SECOND BROWSER TAB, and every gate here opens one.**
  Fourth instance of *a test population you generated cannot contain the shape you are missing*, and
  the newest axis: not an encoding, not another implementation, not a profile — a second *window onto
  the same profile*. Two multi-tab gates existed (`second_tab_detects_secondary_and_warns` and its
  Direct twin) and both assert the **banner**; nothing had ever asked a second tab to do anything.
  `a_second_tab_may_post_as_the_temporary_identity_it_is_running_on` opens one, **presses Post**, and
  **tab 1 is a control that runs first** — without it a run where *neither* tab can author passes
  every row. It deliberately does **not** assert the post survives: an ephemeral tree goes with the
  tab, which is what the caveat tells the person, and durability is
  `a_default_profile_can_post_…_survives_a_reload`'s subject on the arm that has it.
  ⚠ **And it was reported as a regression and was not one.** Nothing in the two commits before it
  touched this path; what changed was the reporter's *environment*, not the code. This file already
  says *"confirm which build the reporter is running before tracing code"*; the addition is
  ⇒ ***when a report says "we're back to getting this", ask what changed around the build as well as
  in it*** — and the cheapest instrument was the one that settled it in four minutes: **drive the
  reported surface in a real browser and then drive it again in a second tab**, rather than reading
  the diff.
- ⭐ **A STANDING FACT IS SAID ONCE; ONLY NEWS IS SAID PER EVENT — and the tell is that the sentence
  does not change (2026-09-17).** Two pieces of copy on this surface repeated themselves at somebody
  who had already read them. *"Posted. Anyone connected to you can read it."* — true, and a property
  of **having a feed at all** rather than of this post, so every press restated it; the reach belongs
  once, in the pane hint, and a confirmation confirms and stops. The temporary-identity caveat was
  drawn above the composer on **every render for the life of the tab**, when it is owed once, before
  they type. ⇒ **ask of every sentence a surface emits: would this have been different if the act
  had gone differently? If not, it is not a report of the act** — and a surface that narrates
  standing conditions at each press trains people to stop reading the line that will one day matter.
  ⛔ **The dismiss reaches the CAVEAT and never the REFUSAL**, which is the only structural part: a
  refusal explains a control that is *absent*, so hiding it leaves a dead box with nothing beside
  it. `compose_caveat()` is a third accessor rather than a third meaning loaded onto
  `can_author`/`compose_note` — *there is something to tell you* is not *and therefore you may not*,
  and *you have not read it yet* is a third fact. Session-scoped: a fresh tab is a fresh temporary
  identity. Gate: `dismissing_reaches_the_caveat_and_never_the_refusal`, enumerated over every arm,
  two neuters landing on two different assertions.
- ⭐ **AN ENUMERATION OF WHAT A MODULE DOES *NOT* CONTAIN IS A CLAIM ABOUT CODE NOBODY HAS WRITTEN YET
  (2026-09-15).** `feed.rs`'s module doc opened *"four of the convention's six types… `collection` and
  `mirror` are deliberately not here"* — while `FEED_MIRROR_TYPE` was defined **twenty lines below that
  sentence** and `feed_mirror.rs` was 1,417 lines. `entity-core-papers` read it and published *"four of
  six"*: their conclusion was right and their evidence was our drifted artifact. **Nothing anywhere
  re-reads a list of absences when the module grows** — the same shape as `window_hydration_census`'s
  `code_only` helper claiming it *"only has to stop the two false positives this census can actually
  produce"*, which is also a statement about the future. ⇒ **prefer *"all N"* to a list of exclusions,
  and where an exclusion is real, say what would retire it.** Second-order: this is the *stale-doc*
  family AP40 warns about, arriving where no gate can see it — a module doc is prose, and
  `publish-doc-check` says in its own output that prose is not checked.
- ⭐ **A DOCUMENT THAT WAS NEVER ADDRESSED TO ANYBODY CANNOT BE UNDELIVERED, SO NO DELIVERY DISCIPLINE
  FIRES ON IT (2026-09-15).** This file already carries *archived is not delivered* and *filed ≠ routed
  ≠ answered*, both written about things that **were** packets.
  `PROPOSAL-2026-09-11-PORTS-…` sat in `docs/plans/` with **zero citations in arch's tree** — not late,
  not held, not lost, just invisible to `spec inbound`, which enumerates `docs/status/ROUTING-*` — while
  the question it answers was asked twice. Arch found it and asked for a one-line stub.
  ⇒ **if a plan answers somebody else's ask, the answer is a packet, not a plan.** Sibling to the
  standing rule that *a held packet is a row on the tracker, not a banner on the packet*: both are cases
  where the artifact exists, is correct, and is not in the channel that anyone reads.
- ⭐⭐ **A BOUND STATED HONESTLY IS NOT A BOUND MEASURED — the pointer body, found by publishing
  somebody else's real corpus and looking at it (2026-09-16, `src/feed_body.rs` + `FeedSource::blob`).**
  EMBED §3 caps an inline payload at `bstr .size (1..16384)`, so **every post over 16 KiB is REQUIRED
  to take the pointer arm**. The publisher had done that correctly since `asset_store::stage`: blob
  and chunks into the closure, `--verify` confirming them. **Nothing on the reader side resolved
  one** — `read_feed` returned the entry, the body was a `Pointer`, and the ladder bottomed out on
  the authored fallback. ⇒ **an 18 KB post arrived on screen as its title, bytes published, signed
  and verified at the origin.** `FallbackReason::PayloadNotInHand`'s own doc called it *"not a
  permanent verdict — a statement about this call site"*, which was true, honest, and had no trigger
  measurement behind it.
  ⭐ **The `app/site-asset` work had already run exactly this measurement one vocabulary over** —
  *629 of 1,346 site assets are over 16 KiB (47%)*, with the conclusion written into this file:
  *before pricing a conformance fix, measure how much production traffic sits in the governed case.*
  Nobody ran it for feed bodies. ⇒ ***a measurement recorded for one subsystem does not fire for its
  neighbour; when you meet a shared constant, grep for who else already priced it.***
  ⛔ **THREE REASONS NO GATE COULD SEE IT, AND THE THIRD IS NEW.** (1) Every feed fixture on either
  seat was a short post, so the arm was reachable only by a test that built it deliberately. (2) The
  one test that did asserted the **publisher's** obligation — the blob is served — and then
  **hand-assembled the closure itself** with a `MemoryContentStore` and three kernel calls, so it
  proved the bytes were there and never read one back through a reader. (3) ⭐ **Every `FeedSource`
  in the crate, doubles included, answered the defaulted *cannot resolve blobs*** — so the production
  gap was invisible from inside the suite. That is *a test double that supplies the thing you forgot
  to ask for cannot notice that you forgot* (fourth instance) **in the direction where it supplies
  NOTHING and both sides agree about the absence** — strictly harder to see than the `OriginFeedSource`
  empty-origin case, where the double supplied a wrong value. ⇒ **when you add a defaulted trait
  method, give the test double the real override or the default becomes the tested behaviour.**
  **`BlobMiss` is the AP40 split one level in.** *We hold no bytes yet* was itself three facts:
  **`SourceCannot`** (ours — no resolver; the defaulted arm) · **`Absent`** (theirs — a signed
  closure naming a blob the origin will not serve, which `--verify` catches) · **`Failed`** (neither
  — transport). The error type on the trait is `BlobMiss` rather than `String` precisely so the
  distinction survives the call, and `data-fallback-miss` carries it to the surface as a **second
  attribute**: *what the reader is doing* and *whose situation it is* are orthogonal axes, and one
  key would merge them.
  ⚠ **The gate's first cut failed honestly and the reason is the transferable half: the fallback
  string was the body's own `# The long one` heading**, so *"the fallback reached the screen"* and
  *"the body reached the screen"* produced the same substring. ***A gate whose two outcomes render
  the same string is not a gate*** — the fallback is a sentinel appearing nowhere in the body now.
  ⚠ **And the classifier was written twice** (shipped `OriginFeedSource::blob`, plus the
  directory-backed `SignedOrigin::blob` the gate runs against), so the tested copy and the shipped
  copy could disagree and both pass. `feed_fetch::resolve_blob_over` is the one expression;
  **the double calls it**, which is what makes the falsification mean anything. C15 in the place it
  is least visible.
  Gates: `a_post_too_long_to_inline_is_read_back_as_markup_and_not_as_its_title` (the whole chain,
  asserted as **markup on screen** rather than as `Resolved` — a blob resolved into a renderer nobody
  called passes a structural check and fails the claim) ·
  `a_source_that_cannot_resolve_blobs_says_so_rather_than_blaming_the_publisher` ·
  `a_blob_the_origin_will_not_serve_is_reported_as_theirs_not_as_our_missing_resolver` (which also
  asserts the feed does **not shorten** and the entry still **verifies** — a body we cannot fetch
  says nothing about whether the author signed it). Two neuters, landing on different gates.
  **Stated bounds:** a **mirrored** long post and one of our **own** posts still show their title —
  `read_mirror`'s scoped source and `feed_compose::own_posts` resolve no closure — both reported as
  `source-cannot`, both unreachable today, both stated in source.
  ⇒ **Standing check when a spec gives you a size, a count or a depth ceiling: publish a real corpus
  across it before you believe the arm above it works.** Ours came from `entity-core-papers` — **2 of
  58 posts** over the ceiling, and they were the two longest, i.e. the subset a reader most wants.
- **AN ADDRESS IS A FIELD NOW — `src/open_target.rs` is the read side of `publish_axes`, and a
  viewer is a row there plus a `WindowView::aim` override (built 2026-09-12).** `Action::SpawnWindow`
  carried a window kind and a peer and **no address**, so a caller who knew exactly what a reader
  wanted to look at had nowhere to put it, and the two callers that had one used a side channel: the
  Registry Browser spawned a hard-coded `("Site Browser", peer)` and warmed manifests *as a side
  effect*, and `BootSurface::Window` could boot the Feed window and not say *which* feed. **The
  privileging was never in the UI; it was in one type.** The address is `APP-CONVENTION-REFERENCE`'s
  atom, whose §3.2 string round trip is load-bearing rather than tidy — a deployment document and a
  persisted config both need a string. Design + what building it corrected:
  `docs/plans/DESIGN-2026-09-12-BROWSING-WITHOUT-PRIVILEGING-A-CONVENTION.md` §5.2.
  **The SUBJECT and the BINDING are two fields because they are two facts** — a window's bound peer
  is the store it READS, and binding a Site Browser to the publisher is the shipped bug
  `views/registry_browser/output.rs:open_target` narrates (a real window, a plausible title, an
  empty rail, because no local SDK hosts that id). One test asserts both peers at once so a later
  tidy cannot collapse them back.
  ⭐ **The aim runs AFTER `hydrate_durable`, and that ordering is the whole correctness argument —
  with no new guard.** `durable_hydration_job` takes its witness **synchronously**, inside the
  hydrate call, so an aim applied afterwards makes the in-flight read report `Superseded` and keep
  what the caller asked for. *An explicit address is newer than a persisted one*, which is what
  asking for it means. Applied **before**, the aim is inside the witness and the persisted location
  lands on top of it. `spawn_offers_the_aim_to_the_window_it_created_and_does_it_after_hydrating`
  records the order of the two calls, because the round-trip itself is `cfg(wasm32)` and no native
  test can see it any other way.
  **`Aim` is four outcomes and three are refusals** (AP40): *this window takes no target* / *took it*
  / *not my convention* / *mine and names nothing I can open*. The last is the Registry Browser's
  **live** case — a resolved name says *whose* sites and never *which* — and reporting it as a hit
  is how a guess gets a feature's name. Count asserted.
  **Still true and now enforceable rather than advisory:** do not add a hard-coded
  `("Some Window", peer)` pair (there is a table; add a row), and **do not let a new viewer INFER its
  source** the way `views/games::app_source` picks *the first foreign entry in the origins registry*
  — *a viewer that cannot be aimed will invent an aim, and the invention is always "the first thing I
  already hold"*, which is AP54. It can be aimed now; nothing hands it an address yet, so the guess
  is still there.
  ⚠ **DISCOVERY is untouched and is the whole of what is open.** *Which viewer handles this address*
  is answerable from the address; *what does this peer publish* is not, and nothing here invents an
  answer. The Registry Browser's `sites` guess survived — it is one visible line now instead of a
  window name in a literal, and the day a binding can say *"I publish a feed"* that is the line that
  changes.
  **`window_target` is a declared deployment field** (`--window-target=<ref>`, refused at the CLI if
  it is unreadable, unroutable, or names a viewer that is not the `--window-type` beside it). It is
  **posture, not routing** on D25's census — a returning reader who has navigated elsewhere must not
  be dragged back by a later publish — and it is deliberately **not** a build knob: an address names
  a runtime peer id and a `const` default cannot bake one, which is `BootSurface::Window`'s own
  existing rule about its peer argument.
  ⭐ **Fourth instance of `spec vocab`'s path/tag confusion, and the first in PROSE: the doc comment
  explaining that a bare `app/…` literal reads as a type tag was itself read as one.** The analyzer
  scans raw lines, so prose about a tag and an emission of one are the same bytes to it. Segments are
  spelled with a leading slash (`open_target::FEED_SEGMENT`) and **do not write a quoted `app/…` into
  a comment in this repo.**
  ⭐ **FIFTH, 2026-09-17, in a TEST CONSTANT — pinning a cross-impl comparand as the whole key.**
  `const MIRROR_KEY: &str = "app/feed/mirrors/{coordinate}"` reds `vocab-lint` with
  **`implemented-undeclared`**, i.e. the analyzer's own costly false direction: accusing a conformant
  seat of inventing vocabulary, over a tag nobody emits. ⇒ **pin the COORDINATE and take the prefix
  from the module that owns it** — which is better test hygiene anyway (the coordinate is what the
  other seat has to reproduce; §6.0.1's prefix is not the test's to restate). ⇒ ***the analyzer
  cannot tell a tree path from a type tag in this corpus and the corpus is not going to change, so
  the durable fix is on the EMITTING side — derive keys, never spell them*** — and note the four
  before it were a trailing slash, a complete path, a parametric family and a comment, i.e. every
  instance has been a different *kind* of line, which is why a rule phrased about any one of them
  keeps not covering the next.
  ⇒ ⭐ **THE `sites` GUESS IS RETIRED — 2026-09-15, `src/publication_probe.rs`** — and not because a
  binding learned to say it. **The publisher can be asked.** Read `open_target::EntryPoint` before
  designing anything that browses, because its two arms are the answer to *"why is `sites` a
  top-level path and the feed an `app/` one"*, and the answer is **inherited, not ours**:
  `APP-CONVENTION-SEMANTIC-CONTENT-SITE` v0.5 §2 makes a site *"a free subgraph at any
  publisher-chosen tree path"* and pins **no tree path at all** — what §11 registers is a **URL
  projection prefix** at NETWORK §6.5.6's demux, *"a publish-time projection, not a tree-storage
  rule"* — while `APP-CONVENTION-FEED` §4.2 pins **two tree paths by hand** and no URL prefix,
  because §4.1 argues a reader holding no reference *has to start somewhere*. So `/{peer}/sites/…`
  is **our** placement choice (`paths.rs` says so) and `/{peer}/app/feed/index` is the convention's.
  ⇒ **a convention that pins an entry point is probed in ONE round trip; one that does not has to be
  WALKED**, which is the cost argument for pinning one, and is why the probe has a `Key` arm and a
  `Prefix` arm rather than one uniform shape.
  ⭐⭐ **The valuable outcome is a VERIFIED NEGATIVE, and it is the reason this is a probe and not a
  registry field.** A field would be a third party's claim about somebody else's tree — AP30 exactly,
  a durable record of a remote assertion with no path back to it — and unfalsifiable at the reader.
  `SignedFetchError::Absent` is *"the key is genuinely not in the signed tree"*, and `EXTENSION-TREE`
  §3.8 R1 keeps the walk arm honest (a withheld interior node is a visible `incomplete_walk`, never a
  correct-complete-shorter answer). ⇒ ***you can only get a trustworthy "no" from the party who would
  have had to say "yes"*** — the same asymmetry `EXTENSION-REGISTRY` §6a.3a leans on for browsing
  names, one tier up. **Eight outcomes**, and the pair a tidy version merges is *they publish none* /
  *we could not tell*; the second is five causes naming **three parties**, with `Budget` kept apart
  from `Declined` because a floor is a policy and a non-converging pump is a defect, and merging them
  files a bug as a setting. **Only `Yes` licenses an Open** — the retired guess handed a feed-only
  publisher a Site Browser whose rail is empty by construction, i.e. a correct registry answer
  rendered as *"this publisher has nothing"*.
  ⛔ **What is still open, and it is the design question rather than a gap here: this scales with the
  number of peers you ask.** Asking a whole registry is O(registry) round trips. `EXTENSION-REGISTRY`
  §3 answers *who and how to reach them*; §3b answers *what shared **infrastructure** a deployment
  offers* — **there is no third record for what a peer PUBLISHES**, and FEED §9.5 names the same hole
  from the content side (*"the aggregation half is solved by §6, the navigation half is open"*).
  §6's mirror **is** the aggregated answer; what is missing is how a reader learns a gatherer exists.
  Do not invent either here — the precedent to cite is §3b itself, whose own preamble records all
  three impls building a pool-selection rule with no protocol way to learn a pool.
- **A LIVE PEER READ IS THE SAME READ, AND THE TWO THINGS THAT ARE NOT THE TRANSPORT ARE THE
  ENTRY POINT AND THE PERMISSION — `src/feed_peer.rs` + `src/remote_read.rs`, 2026-09-11.**
  Asked whether a connected peer could do *"these same operations"*, since *"the difference
  should be pretty minor, it's just the transport"*. Right about the cost — `PeerFeedSource` is
  ~40 lines and `read_feed` runs unchanged — and the measurement found two things under it.
  Full review: `docs/plans/REVIEW-2026-09-11-…`; the surface census (who reads over which
  transport) is its §1 and **the two halves of the product were disjoint**: chat and file
  transfer live-only, sites/apps/feed origin-only, nothing crossing.
  **⭐ THE INDEX IS A PUBLISH ARTIFACT, NOT A TREE ARTIFACT, AND NOTHING IN THE PRODUCT SAID SO.**
  `index_head_path`/`index_page_path` — the absolute tree paths — are referenced by **nothing in
  this crate but their own unit test**. `feed_ingest` binds entries at `app/feed/entries/{hex}`
  and writes no index; `plan_index` builds head and pages **at publish time**, into the out-dir.
  So a key-resolving live reader asks a peer with three posts for `app/feed/index`, gets a 404,
  and reports they have no feed. **The transport was never the missing piece** — and this is
  invisible from either side, because the publisher builds the index on the way out and the
  reader had only ever read a published tree. ***When two paths share a key space, ask which of
  them CREATES each key*** — identical addressing is not identical content.
  Closed with §4.3 rule 6's own fallback (*"enumerating the prefix — slower, same answer"*),
  which `entry_prefix`'s doc already said it existed for and which had had no consumer.
  `FeedSource::list` is **defaulted** to `Ok(None)` = *this source cannot enumerate*, so the HTTP
  arm is byte-identical and adopts it later by implementing one method (`SignedSession::enumerate`
  exists and is bounded). **Three facts, not two**: cannot-enumerate / enumerated-and-empty /
  could-not-look. Two bounds stated: the prefix is **ours** not the convention's (`A-38`), and the
  **order is reconstructed** — §4.5 makes it authored and the index is what carries it, so the
  fallback reuses `feed_tree::sort_key` and logs its arm rather than presenting the two as
  interchangeable. **The better long-term answer is to write the index into the tree at ingest
  and let the publish project it** — then §0.1's model holds with no exception and no fallback —
  and it is a design question (`plan_index` takes a page size and a clock) rather than something
  to smuggle in under a transport change.
  **⭐⭐ A GATE WRITTEN TO ASSERT A REFUSAL FOUND THAT NOTHING REFUSES — AND MY READING OF THE
  DEFAULT WAS ONE LAYER TOO LOW.** The module doc said a live read is 403 until shared, on
  `default_connection_grants` (`system/tree:get` at `system/type/*` + `system/handler/*` and
  nothing else). The two-peer gate asserting that **failed: an ungranted reader got the whole
  feed.** Cause, measured with a three-path probe (app-private / granted / `system/peer/keys`)
  that returned **no 403 at all**: `PeerManager::with_keypair_and_optional_connector`
  (`bindings/sdk`) builds every peer with `debug_open_grants: true`, overriding
  `PeerConfig::default()`'s `false`. **I read the kernel's default and concluded the posture** —
  *a correct instrument one layer below where the value is set is a persuasive way to be wrong*,
  and the probe that settled it cost four minutes. ⇒ **a peer you are connected to can read any
  path in your tree.** Not new — `share.rs` has said *"enforcement is still off"* since the SHARE
  work — but stated as a **write** there and as a **read** here, which is the direction it is a
  *disclosure* rather than an inert grant, and the direction someone asking *"can I read my
  friend's feed"* arrives from. ***When a doc says enforcement is off, ask what that means for
  READS, not only for the grants you are authoring.***
  **What it cost in gates is the transferable half, because the honest answer was not "assert it
  anyway".** The refusal is **unreachable**, so: the 403 → `NotShared` mapping moved into a
  **pure** `remote_read::classify` (a two-peer version of it is a test that cannot fail); the
  positive read gate **does not author a share**, deliberately, because with open grants the read
  is identical either way and sharing first would read as *"the grant is what made this work"*
  while measuring nothing — a gate satisfied by its fallback; and
  `the_live_read_needs_no_grant_today` **pins the posture at a path no share covers**, exists to
  **go RED when enforcement flips**, and says in its own doc comment to invert it rather than
  delete it. **Stated: that one has no falsifier** — flipping the flag means editing a sibling
  repo — so it is an observation with a tripwire, not a proven property.
  **`remote_read` is the extraction, made when the second caller proved the boundary.** Every
  line of `file_offer`'s `remote_execute`/`resource_opts`/`empty_params` was about a **remote
  read** and none about a file; `classify` also removed a four-arm match that `read_entity_at`
  and `list_keys_at` had duplicated. Not a standalone rename — the charter's rule, and EMBED's
  payload union is the precedent.
  ⇒ **The window is WIRED as of 2026-09-11, and the framing was the thing that was wrong — see
  `src/feed_route.rs`.** *Live or published* is not the question: a publisher may serve their feed
  at only one of the two, so it is a **priority list**, and **an empty answer from one leg is not
  evidence about the other**. A live peer with nothing in their tree beside a published tree full
  of posts is what a publisher who cannot carry the load looks like from here — stopping at the
  first leg that *answered* reports *"this author has posted nothing"* one hop from their archive
  (AP54's family). **Serving stops the walk; answering does not**, and the gate asserts the second
  leg was **never consulted**, because *"the answer came from the first"* passes with no
  short-circuit at all. **A publisher cannot state a preference — measured**, and routed as
  `A-43`: FEED's only use of *live* is §2.2's live-**reference** atom, a different axis.
  **The default (live-first) is the weaker read today and the module doc says so**: the index is a
  publish artifact, so the live leg falls through to the prefix fallback, which *reconstructs* the
  order where the published leg *reads* it.
  **No browser gate**: the live gates are native over the memory transport, so *"the
  transport does not matter"* rests on `DispatchHandle` having no branch below it — a structural
  argument, not a measurement. **Pointer bodies are not wired** (the blob walk is
  `file_offer::pull_offer`'s, one consumer over; `system/content:get` resolves **by hash, not by
  namespace** — the namespace is the capability scope).
- **THE GATHERER IS BUILT — `src/feed_mirror.rs`, `APP-CONVENTION-FEED` §6 — AND ITS CLOSURE GATE
  CANNOT SEE THE DEFECT IT LOOKS LIKE IT GUARDS (2026-09-11).** Arch ruled `D20` (closure's two
  preconditions) and asked for this first. **The shape is forced, not chosen:** the gatherer signs
  the **mirror record** and the carried entries are written at their **own author's** addresses and
  are **not** in the gatherer's signed root — `RootProjector::record` skipping a foreign peer is the
  tree tier saying a root commits only to keys under its own peer, and the entries stand on a pin
  plus the author's detached signature instead. That is what *author-anchored evidence surviving
  detachment* has to mean, and it is why `entry_key` being the entry's own hash matters: the
  consumer's address is derivable, so `read_mirror` and `read_feed` share **one** `finish_entry`.
  ⚠ **THE A→B→C GATE IS GREEN UNDER THE NAIVE-REPUBLISH NEUTER.** Every fixture in it came from our
  own encoder, so decode-and-re-encode is lossless over them — `APP-CONVENTION-FEED` §6.1 names that
  hazard by hand (*"a round trip through bytes your own encoder produced proves nothing"*) and our
  first control arm walked into it and failed honestly. The arm that measures the `MUST` needs **a
  field the reader does not declare** (V7 §2.6 obliges us to ignore exactly those), authored and
  signed by the publisher: then the hash moves, **nothing errors anywhere**, and the signature stops
  naming what was bound. ⇒ ***a gate whose fixtures your own encoder produced is testing your
  encoder against itself*** — a fourth face of *ask what your gate's expected value depends on*, and
  the gate's own doc comment says which of the two measures the property.
  **`Entity.content_hash` is a CLAIM, not a check.** `SignedSession` verifies it on the published
  leg; a live peer and a republishing peer do not, and both are legs this convention adds. So
  `feed_read` **computes** the address and compares it to the pin (`FeedReadError::Substituted`,
  fatal — §6.1 rule 2's *a source may omit, never substitute*), and `plan_mirror` re-checks rather
  than inheriting, because it is the act whose correctness depends on it (AP44).
  **Two findings routed, both from building rather than reading** (`A-57`/`A-59`): §2.2 makes
  `reference` pinned-only, so §6's `subject` pins **one entity** — a thread — while the replication
  proposal's closure trace republishes **timelines**, which are growing prefixes and cannot be
  pinned, ⇒ **the third `SOURCE` leg that is the 250× has no type to ride on** and `feed_route::Leg`
  stays at two variants. And a mirror is **not** consumed by the identical code path a feed is: at
  the entry it literally is, at the set it is two walks, so the fixed point closes `mirror → mirror`.
  **Stated bounds:** no verb publishes a mirror, nothing durable is held (D24 not engaged), and the
  run is 4 entities.
  ⇒ **BOTH ruled our way 2026-09-12 and the SECOND one made the `MUST` stricter — read
  `feed::MirrorSubject` before touching a mirror key.** `subject` widens to `any-reference`
  (`A-57`), and the layer measurement became `DX-R4`, a **`MUST NOT`**: never publish, under our own
  namespace, an author's own set-layer object over content that author did not place there. We had
  filed that one under *"we do not think this weakens the design"* — true, and the small half. **It
  was building a forgery**, and every byte of it verifies. ⇒ ***a sentence true at the layer everyone
  is thinking about and false at the layer nobody is reviews clean forever*** — three review rounds
  and one build round agreed with it, and running a consumer against a gathered view broke it in an
  afternoon. **When you measure that a normative sentence holds at only one layer, the finding is not
  "it still works" — it is what a reader who believed it would build.** `DX-C6` gates it.
  **`FEED-R26`/`R27` are enforced by ARGUMENT LISTS, not validators**, because nothing in a mirror's
  bytes says whether its pinned subject moves: `MirrorSubject::timeline` takes **no hash**, so the
  forbidden subject is not expressible, and `from_reference` names **every field of both atoms** with
  the hints bound to `_` — so adding a field to the reference atom is `error[E0027]` at the one site
  that decides what a coordinate is made of. *A `..` there is the version that silently starts
  ignoring a field somebody later decided was identifying.*
  ⚠ **§6.0.1's live key is ONE ARGUMENT SHORT and it is the comparand of its own vector** (`A-60`,
  `A-61`): `content_hash` is V7 §1.4 over `{data, type}` and the clause supplies a path, which is not
  an entity; and §6.0 never says *which* path *"an author's feed"* is. Two readings, different bytes,
  and `FEED-12` compares exactly this — so no seat can pass it from the text. ⇒ **when a clause names
  a function the corpus defines with more arguments than the clause supplies, the spelling is the
  finding, not a detail to settle locally.** Ours is `sha256(utf8("/{peer}/app/feed/index"))`, pinned
  to a literal computed with `hashlib` rather than with the function under test.
  **`Leg::Mirror` exists and is NOT reachable from a browser, and the reason is the trust argument
  rather than the wiring** — the carried bodies are deliberately outside every signed root at the
  gatherer's origin, so an HTTP mirror source cannot be the `SignedSession` walk every other foreign
  read here uses. **Our native double could not see that: a map keyed by `(peer, key)` verifies
  nothing.** Same shape as `OriginFeedSource`'s empty origin — *a test double that supplies the thing
  you forgot to ask for cannot notice that you forgot.* And **a mirror axis is not a row in
  `publish_axes`** — `REFERENCE-PUBLISHING-PIPELINE` §0.2a is why, including that `--verify` sweeps
  our own peer only and therefore cannot see a mirror's carried bodies at all.
  **Rig lesson, paid for here: the R26 gate's first cut used 3 → 4 posts and failed its own
  anti-vacuity assertion**, because `published_tree` publishes at a **fixed** clock — so the head's
  `updated_at` cannot move and only a page boundary changes it. *A rig that holds a variable still
  cannot measure a property that is about it*, and this is the cheap direction of that lesson: the
  gate said so instead of going green.
- **A READER-OWNED REFUSAL IS NOT A VERIFICATION FAILURE, AND OURS WAS BLAMING THE PUBLISHER FOR
  OWNING TWO COMPUTERS (2026-09-11).** `SignedFetchError::Verify` carried the anti-rollback floor
  and **enumerated the collapse in its own doc comment** — *"bad signature, wrong key, `seq`
  rollback, or a body that does not hash to its address."* Three of those are the publisher failing
  to prove themselves; the fourth is a root that **proved itself perfectly** and that our own
  monotonicity policy then refused, necessarily *after* verification, because a rollback is a
  correctly-signed root being replayed. So a reader saw *"verification failed: seq rollback"* — and
  the field case that produces it is **the publisher's own second machine** (`multi_device_sequence`,
  the `C-2` measurement). *Their laptop is not an attack and the report said it was.*
  `SignedFetchError::Declined` is the split, through **one** `classify_root_error` because there
  were two call sites with the identical collapse (C15) — and it is the only place the kernel's error
  taxonomy is read for **attribution** rather than for control flow, so a new kernel variant lands
  there and nowhere else. **Assert the discrimination, not the relabelling:** `SignatureInvalid`
  still lands on `Verify`, so a split that made every terminal outcome the reader's own would red.
  ⇒ **when a terminal outcome is a policy you chose, it is yours and not theirs** — AP40 where the
  cost is not the merged value but the wrong destination. `AssetPayload::Unsupported { tag }` is the
  same row with a different owner (capability rather than policy), and both are second-seat evidence
  for the replication proposal's seventh outcome.
- ⭐ **THE SITES AXIS HAD A DEFAULT AND THE DEFAULT INVENTED CONTENT — three arms now, and silence
  is refused (2026-09-16, `SiteSource` / `parse_site_source`).** `--ingest=<dir>` / `--demo-sites` /
  `--no-sites`, exactly one; `--verify` is exempt because it resolves no source. **The account is
  `REFERENCE-PUBLISHING-PIPELINE.md` §0.2, not here** (AP56's corollary). Two sentences worth
  carrying out of it: **the cost of an invented default is decided by what the operation DELETES**
  — the clean is wholesale, so a publish that meant `--ingest` and omitted it *replaced* a domain's
  real sites rather than adding to them, exit `0`; and **the argument against it was already in the
  file, one axis over** (`--ingest-feed`'s *"a publish that invented an empty feed would claim every
  site publisher has one"*), pointed at its neighbour by nobody for two months. ⇒ ***when you write
  down why an axis must be opt-in, read it back at the axis next to it.***
- **THE THIRD PUBLISH AXIS SHIPPED, AND `src/publish_axes.rs` IS NOW *THE* LIST OF WHAT ENTERS A
  PROJECTION (2026-09-10).** `feed_tree::read_owned_feed` reads a peer's own feed off the tree,
  `feed_ingest::ingest_path` writes an authored `posts/` dir into it, `publish --ingest-feed=<dir>`
  is the front door, and the emit goes through the **same `RootProjector`** a site publish uses.
  **The model is in `REFERENCE-PUBLISHING-PIPELINE.md` §0.1–§0.2, not here** — AP56's corollary: the
  fix for *"the model was not where I looked"* is not to state it again where I looked.
  **A fourth L5 convention is a row in `axes()` plus an `impl PublishAxis`**, and the compiler
  enforces the four obligations. The residue that stays per-convention is tabulated in that module's
  doc rather than left to be rediscovered.
  **Four things closing the loop found that three phases of codec and every prior review did not** —
  and every one lives *between* modules, which is the population argument for building the consumer:
  **(1)** `publish_feed` wrote the entry and **not its body's blob**, so a post over EMBED §3's
  16 KiB ceiling published a dangling reference. The inline arm is *refused* at that size, so the
  pointer arm is the only conformant way to carry a long post — the closure is a requirement of that
  arm, not a nicety. Every fixture in the module was all-inline: *a test population you generated
  cannot contain the shape you are missing.*
  **(2) `--verify` had AUDIT F8's hole again, one convention over.** A heuristic scan filters on
  presence and therefore cannot see absence, which is why an `app/site-asset` is **decoded** rather
  than scanned; an `app/feed/entry` declares the identical pointer and owed the same arm. Falsified:
  without it, a tree whose post has no body exits **0** saying it is clean — and it is worse than the
  asset case by the amount a post is more than a figure. ⇒ **every DECLARING type owes an arm in
  `run_verify`, in the commit that introduces it.**
  **(3) The head's clock is the CALLER's to supply, and `SystemTime::now()` would put a wall clock in
  the trie** — the site root moving on every run, and `G-PIN-4`'s *one fixture, two publishers,
  identical root* comparand unreachable for any tree carrying a feed. It is the feed's own high-water
  mark (`publish_axes::head_clock`). Same defect `system/peer/published-root`'s `published_at`
  already has. **`plan_index` is unchanged and still documents the head as "the publish instant" —
  the parameter is right; who fills it is the decision.**
  **(4) `publish_feed` takes no `prefix`** where the two emitters beside it do, so the axis joins the
  prefixed base itself. Backwards, a whole archive lands outside the hosting scope its own signed
  root is served from. Gated.
  **`created_at` is REQUIRED and refused when absent** rather than read from the file's mtime, which
  `git clone` rewrites — the same posts would otherwise hash differently on every machine, moving a
  page boundary and rewriting the archive. Four spellings, **one parser** (`toml::value::Datetime`),
  vectors from `date -u` and not from the code under test.
  **A tie-break that changes no answer today is still worth having, and say so rather than gating it
  falsely.** Entries sort by `(created_at, content_hash)`; the prefix scan already collects into a
  `BTreeSet` of hex-keyed paths and `sort_by` is stable, so **through the reader the tie-break is
  unfalsifiable** — it is gated against the *comparator* instead, where two input orders must give
  one output. What it buys is that the total order is a property of the comparator rather than an
  emergent consequence of a collection type and a stability guarantee. ***A test that cannot fail is
  worse than no test; move it to the level where the property is real.***
  **Stated bounds:** the **window** still cannot post (authoring is on disk); a **backdated** post
  shifts every entry after it and rewrites the archive from there (appending touches only the last
  page); the feed-and-no-site branch in `run()`'s emptiness check is **unreachable from the CLI
  today** and is marked as such; and `run_plan`'s per-unit *naming* is still per-convention — every
  axis owes a *term*, only the wording is bespoke.
- **THE FOURTH AXIS IS A MIRROR AND IT *IS* A ROW — `publish --gather=<peer>@<dir>`, 2026-09-12.
  The publishing facts live in `REFERENCE-PUBLISHING-PIPELINE` §0.2a, which this replaces rather
  than restates (AP56's corollary); read that before touching a gather, a mirror or the clean.**
  It answers the three questions that section used to leave open, and **one of them the other way
  from how it was posed** — it said `--verify` could not be fixed by an arm; it can, and the arm
  found a real defect on its first run. Four things here that are *not* pipeline facts:
  ⭐ **ENUMERATING TO DECIDE WHAT NOT TO REMOVE IS SAFE; ENUMERATING TO DECIDE WHAT TO REMOVE IS
  NOT.** The content clean had to learn about foreign trees it cannot identify (a gathered author's
  segment and a co-hosted publisher's are the same shape on disk). Asking *"is anybody else here"*
  and only ever answering *"then leave it alone"* is sound — worst case, orphans — while the same
  predicate authorizing a delete is AP52/AP53. **A predicate too weak to delete with can still be
  strong enough to protect with**, and that asymmetry is worth reaching for whenever a guard is
  blocked on *"we cannot tell these apart"*.
  ⭐ **A NEW OBLIGATION ON A TRAIT GOES IN UNDEFAULTED.** `PublishAxis::carried_peers` is right for
  all four rows today and three of them answer `Vec::new()` in one line; defaulting it would have
  been correct for every axis that exists and silently wrong for the next one that is not (AP44).
  `error[E0046]` is the enforcement point; a default is a rule with none.
  ⛔ **OPEN, FOUND 2026-09-14: `tree_prefix` HAS NO CONSUMER OUTSIDE ITS OWN CENSUS, AND THE FEED
  AXIS WRITES OUTSIDE ITS OWN DECLARED PREFIX.** The trait doc calls it *"what the wholesale clean
  removes, and therefore what a publish carrying none of this axis silently deletes"* and the module
  doc opens with *"the clean is wholesale, so `tree_prefix` is load-bearing"* — but the clean is
  `clean.push(base.join(peer_id))`, the whole peer subtree, and **`tree_prefix` is read by nothing
  but `every_axis_names_the_tree_prefix_the_clean_would_remove`.** Meanwhile `FeedAxis` declares
  `app/feed/` while `publish_feed` also writes `system/signature/{hex}` (`FEED-R2`'s invariant
  pointer), so the census asserts something untrue of that axis. **Inert today** — the wholesale
  clean removes the signatures correctly, by a mechanism that never consults the declaration — which
  is the *whole* reason nobody noticed. *A value that is only printed is not a guard*, second
  instance after `--prune`'s `assets_named_by`, described in its own doc as *"the whole safety
  argument"* and wired to nothing. ⇒ **the day anyone scopes the clean to the declared prefixes —
  which is what the declaration is FOR — `app/feed/` is cleaned and rebuilt while
  `system/signature/` is left behind unbound by the new root, and every entry in a statically
  published feed becomes unattributable.** Found by `entity-workbench-go` reading our source and
  reaching that conclusion about *today's* code, where it is wrong (`RootProjector::record`
  early-returns only for a **foreign** peer, so an own-feed signature is committed to the root —
  measured, `a_published_feed_reaches` reads 34 of 34 attributed). **Their conclusion was right
  about a system; it was the wrong system and the wrong decade.**
  ✅ **CLOSED 2026-09-15, and it did NOT close by adding the missing prefix — it closed by making
  the declaration MEASURED.** `tree_prefixes` is plural (an axis writes more than its own
  convention's namespace: the feed borrows V7 §3.5's invariant pointer rather than owning it), and
  `every_key_an_axis_records_falls_under_a_prefix_it_declared` runs each axis's `project` against a
  real projector and compares the keys it **recorded** with the prefixes it **declared**. The old
  census asserts what the table *says*; this one is satisfied only by its being *true*, and it found
  the false row the first time it ran. Falsified both ways — drop the prefix and it reds naming the
  key; hand an axis nothing and it reds on anti-vacuity. **Adding the row alone would have left the
  next axis with the same hole**, which is the whole difference between the two fixes.
  ⭐ **This is the promotion the shape had earned and arch called it before we did**
  (`ROUTING-2026-09-15-a` §4): *a second instance of a shape in one tree is when it earns a check
  rather than a note.* **A guideline restated in a second doc comment is a third instance waiting to
  happen; the ladder's answer to "it bit us twice" is an enforcement point, not a better sentence.**
  ⭐ **THIRD INSTANCE OF *GREP YOUR TEST SCAFFOLDING FOR THE FIX*.** `feed_read::block_on` was
  `#[cfg(test)]` with a doc comment saying *"native tests only"* — and a native CLI verb driving an
  async source over synchronous file reads is exactly what it does. It is `cfg(not(wasm32))` now, so
  **the cfg is the guarantee and the doc comment is only the reason**; the browser still cannot
  reach it. Same session: `feed_publish::tests::Origin` was the **fourth** directory-backed
  `BinSource` in this crate and is now the production one (`feed_gather::DirOrigin`), with the other
  three named rather than folded — that consolidation belongs to whatever proves the boundary.
  ⭐ **THE HANDOFF'S OWN WARNING, DISCHARGED: *a map keyed by `(peer, key)` verifies nothing*.**
  `feed_fetch::OriginMirrorSource` is the production consumer, and it is **two-legged because the
  two halves of a mirror stand on different evidence** — the §6 record through the gatherer's signed
  root, every carried entry by two-hop address plus the record's pin plus the author's detached
  signature. `RootProjector::record` skips a foreign peer, so the carried bodies are *by
  construction* outside every signed root at that origin: a source resolving them through the
  session gets `Absent` for all of them and reports a healthy mirror as empty. **A double that
  answers both legs identically leaves the whole trust argument untouched by every assertion.**
  **Stated bounds:** `--gather` reads a **directory, never an origin** (no native HTTP client in this
  tree — `feed_gather`'s module doc leads with it); **timeline subjects only**, because a CLI flag
  naming a peer cannot express a thread's entity hash; the browser cannot gather (no out-dir, `F3`);
  and **no window shows a mirror yet**, which is still the largest gap on both seats.
- ⭐⭐ **CORRIDOR ① — AND THE VALUE WAS THAT IT DID NOT READ. A FIXTURE YOUR OWN PUBLISHER WROTE
  CANNOT FALSIFY ANYTHING YOUR PUBLISHER AND YOUR READER AGREE ABOUT (2026-09-15,
  `src/crossimpl_feed.rs`).** `entity-workbench-go` cut a feed fixture twice (peer root /
  `app/feed/`) and we took the reader role: the first time anything in this tree read a feed
  somebody else published. It took **two fixes to get the first byte**, both in
  `feed_gather::PublishedTree` — the source `publish --gather` uses — and **both fatal at the walk**:
  the layout was **derived** rather than read (`{peer}/system/peer/published-root` vs their
  `{out}/manifest`, hop 0 every time), and §4.2's **pinned absolute address** went verbatim to a
  **prefix-relative** resolve (their narrow cut commits `index`; we asked for `app/feed/index`).
  ⛔ **`publish_layout.rs` had asserted the first one was possible the whole time.**
  `two_conformant_publishers_advertise_different_manifest_urls` compares *their* manifest URL with
  *ours* and says in its own body *"if these ever coincide this gate has stopped testing anything —
  the point is that a convention-derived front door misses a conformant peer."* `DirFetcher` reads
  the profile; the gatherer was written later and derived one. ⇒ **a test that proves a hazard is
  real does not stop the next module walking into it** — the rule needs an enforcement point, which
  is now one expression (`PublishLayout::advertised_for`, carrying AP52/AP53's peer guard) that both
  callers take.
  ⭐ **The population argument at its strongest form: the missing shape was ANOTHER IMPLEMENTATION.**
  Every `--gather` fixture here is a directory our own publisher wrote, so the layout the gatherer
  assumed and the layout the fixture used were *the same expression*, and our emitter has never
  declared a narrow prefix. That is `OriginFeedSource`'s empty origin one convention over and a
  degree stronger — *a test population you generated cannot contain the shape you are missing*, and
  here no amount of care inside one repo produces it. **There was no cheaper way than a real foreign
  emission**, which is the argument for vendoring the counterpart's bytes as a standing gate rather
  than running them once.
  ⭐ **THE STRIP BELONGS TO THE SOURCE, NOT THE READER.** A convention pins **one** address; what
  varies is the transport the bytes arrived over, and a **live peer has no declared prefix at all**.
  `SignedSession::{declared_prefix, key_under_prefix}` is the one expression, called from
  `PublishedTree::get`, so `feed_read` keeps spelling §4.2's address the way §4.2 spells it.
  `entity-workbench-go` reached the same placement from the other end (`fetch.AbsolutePath`), which
  is when a shape is worth believing. Their framing of the hazard is the one to keep: *getting it
  wrong yields an empty feed with a valid signature over it, which is the most confident wrong
  answer available.*
  ⭐ **OUR SYMPTOM WAS BETTER THAN THEIRS AND ONLY BY LUCK — AND A LOUD FAILURE IS NOT A DIAGNOSIS.**
  They got 34 correct entries via §4.3 rule 6's fallback with every caller-visible field right
  (their `AP106`); we got a hard `NoIndex`, **because the published leg has no fallback to rescue
  it** — `FeedSource::list` defaults to *cannot enumerate* and `PublishedTree` does not override it.
  Neither report named the cause. ⚠ **Do not read that as a virtue we designed**: it is
  `SignedSession::enumerate` not being wired to that trait, and **`feed_peer` (live) DOES implement
  `list`, so this seat can still have their defect there.** Their `B-14` — *report how many entries
  you reached via the index and how many via the fallback, not just the count* — is answered
  **34 / 0 on both cuts**, asserted in the gate rather than noted in a comment so that wiring
  enumeration in re-measures the claim instead of inheriting it.
  ⛔ **`FEED-14`'s negative arm asserts `NoSignature` SPECIFICALLY, not merely *not attributed*.** A
  reader reporting a publisher's scoping choice as `BadSignature` or `SignerIsNotTheAuthor` makes a
  statement about **the author** to cover **the publisher** — which is `A-36` §1.2's own argument for
  the rule, arriving as a test rather than as prose.
  ⚠ **And the defect the fix itself introduced, caught by `make test` rather than by reading:**
  re-rooting their layout onto a directory dropped the leading slash, and `prefix_carries_peer` is
  `rsplit_once('/')` — so a bare `{peer}` reads as *not peer-rooted* and every tree URL came out
  `{peer}/{peer}/…`. The whole `--gather` suite red with *"signature pointer: origin served no such
  entity"* — **the origin blamed for a string we mangled.** Pinned by
  `re_rooting_a_layout_onto_a_directory_does_not_double_the_peer_id`, and asserted **through
  `tree_leaf_url`** rather than on the field: *the field's spelling is not the property, the URL a
  consumer fetches is* — a test on the string would have passed both ways.
  **Fixture:** `tests/fixtures/crossimpl-go-feed/` (their `779616d`, both cuts, provenance README).
  **It is a wire artifact** — regenerating it is a wire event, not a test fix. Four neuters, each
  landing on a distinct row: derive the layout (all four red at hop 0) · skip the strip (**only the
  two narrow-cut gates**, so the two defects are separately measured) · attribute an unsigned entry
  (only the negative arm, `34` vs `0`) · stop at the page the head names (`2 of 34`).
  **Stated bounds, theirs and ours:** nothing about the live road, nothing about two peers, nothing
  about a publisher that changes between reads — and their own readback is **not** cross-impl
  evidence (one encoder, one decoder, both theirs), which their gate header says. Ours is the
  reader half.
- ⭐⭐ **CORRIDOR ③ — OUR HALF OF `FEED-12`, AND THE FINDING IS ABOUT WHAT A CORRIDOR CANNOT MEASURE:
  ANOTHER IMPLEMENTATION'S BYTES ARE A DIFFERENT POPULATION, NOT AUTOMATICALLY A DIVERGENT ONE
  (2026-09-17, `src/crossimpl_feed.rs`).** We gathered corridor ①'s author through the shipped
  gatherer, pinned §6.0.1's derived key as the cross-seat comparand, and wrote the closure gate's doc
  comment claiming that — at last — §6.1's byte-preservation `MUST` was measured cross-impl, since
  `feed_mirror`'s own A→B→C says against itself that every entry in it came from our encoder and
  §6.1 names that hazard by hand (*"a round trip through bytes your own encoder produced proves
  nothing"*).
  ⛔ **Ran the neuter — carry `row.entry.to_entity()` instead of the obtained bytes — and it came
  back GREEN.** It reds only the gate built on a **synthetic** entry carrying a field the reader does
  not model. Our decoder round-trips every entry in their fixture losslessly **because both seats
  model the same fields**, which is the corridor *working* and is exactly why it cannot falsify the
  rule written for when they do not. ⇒ ***the property that repairs "my own encoder wrote the
  fixture" is an UNKNOWN FIELD, not a foreign author*** — so a cross-impl fixture needs one authored
  by *them*, one flag at cut time, the same shape as `FEED-14`'s two-cut ask. Routed; until it
  exists, that `MUST` is measured by a synthetic entry and by nothing cross-implementation.
  ⭐ **The transferable half is bigger than the fixture: "we ran it against another implementation"
  reads as coverage of every cross-implementation property, and coverage is per-PROPERTY.** Ask what
  the two populations differ in, not whether they have different authors — and the cheap instrument
  is the one that caught it here: **neuter the rule you are claiming to measure, in the gate you just
  wrote, before you write the sentence claiming it.** Same family as *if nothing you opened
  contradicted you, you did not run a check*, applied to a gate's own passing.
  **Also built, and both were doc claims until this session:** `FEED-13`'s (a) *the head's encoded
  size does not follow the member count* — byte-identical heads for a 4- and a 40-member view of one
  subject at one instant, with the membership asserted to differ and the pages asserted to carry it;
  and (c) *a reader fetches the head and one page and stops*, which was asserted on **entries
  returned** and is therefore satisfied exactly by a reader that fetches every page and truncates.
  Counted at the source now (scoped to the mirror prefix, or it measures membership), with the
  whole-view read beside it as the control — **a count that cannot go up measures nothing** — and the
  pre-existing entry-count gate stays **green** under the neuter, which is how we know it was
  unmeasured rather than redundantly measured.
- ⭐⭐ **CORRIDOR ② IS THE *AUTHORING* BOUNDARY, AND A COUNTERPART'S GATE BEING LOUDER THAN THE
  AUTHORITY IT RESTATES IS A DEFECT IN THE AUTHORITY (2026-09-16, `entity-core-papers`).**
  Corridor ① is somebody else's *wire bytes* through our reader. This is somebody else's **authored
  source** through our ingest: their 58 posts across four domains, every one accepted, each domain
  publishing and `--verify`ing clean — and `entity-core-protocol`'s 34 crossing §4.3's page boundary
  at 32, which makes it **the only real-corpus feed on either seat that exercises paging**, arranged
  by nobody. `--ingest-feed`'s whole fixture population was `examples/entity-demo/feed/`, which we
  wrote. *A test population you generated cannot contain the shape you are missing*, one layer
  further out than the wire: here the missing shape is **another seat's authors**.
  ⭐ **The finding is the direction nobody looks — they were louder than us.** `tools/feed/emit.py`
  restates our refusals at the seat where the author stands and reported a loss our own ingest was
  silent about: `params` is EMBED §3's open bag and is strings only, so `tags = ["a", "b"]` — the
  shape an author reaches for — went nowhere *quietly*, at the one boundary where the person who can
  fix it is standing. Fixed (`ce91d6fe`): `ingest_path` returns `Ingested { posts, notes }` rather
  than a count, which is meta's `B-5` reason one verb over — *a count alone lets a caller report a
  clean `ingested N post(s)` over a set that quietly lost something.* **Reported, not refused**:
  `created_at` is fatal because it has no correct answer without the author; an attribute the bag
  cannot carry has an obvious one. ⇒ **when a counterpart builds a gate against your behaviour, read
  what it says and not only whether it agrees** — the row where they warn and you do not is yours.
  ⚠ **AND THE RESTATEMENT IS C15 WITH NO COMPILER ANYWHERE IN IT.** Two expressions of one rule
  across a repo boundary, gated in neither direction; six weeks produced two places their gate
  **accepts a post we refuse**, both measured with identical bytes on both sides, both latent in
  their corpus. A false accept is the one direction that defeats a gate whose purpose is to keep the
  refusal away from devops' publish. Their transferable half is worth keeping even though the defect
  is theirs: **handing a string back to a DOCUMENT parser is not handing it to a VALUE parser** —
  `tomllib.loads(f"created_at = {value}")` comment-strips the string's own contents — and their doc
  comment claimed the property the re-wrap breaks. Ours gets it from one `toml::value::Datetime`
  `FromStr`; *the shape is the fix, not the sentence.* Routed with three options and no enforcement
  point chosen: `ROUTING-2026-09-16-b-…` §3, tracker `PA-4`. **`vocab-lint` is the only precedent
  here for a gate that reads another repo**, and its rule applies if one is built — a baseline of
  names fails both ways, a baseline of counts cannot see a swap.
  ⛔ **Bound as written on 2026-09-16 — *a visitor to a published domain cannot FIND the feed,
  reaching a domain's own posts means pasting a 45-character peer id* — AND IT WAS RETIRED THE SAME
  DAY, BY THE BROWSE LIST, WHICH DID NOT COME BACK AND SAY SO.** `origins::list_origins` is the set,
  `adopt_deployment_origin` puts the deployment's own publisher in it at boot, and
  `effective_selection` reads that publisher with nobody choosing. The peer-id box is the escape
  hatch now. **Noted here rather than deleted, because the failure is the charter's own ratchet rule
  missing a beat: *when you land something that spends a stated concession, grep for the sentence it
  retires* — two entries in one file, one session apart, and the newer one never looked.** A bound
  that outlives its cause is the expensive direction (*a shipped capability recorded as deferred
  reads as work still owed*), and it is exactly what a later session prices work off.
  **What is genuinely still open is one step further out: DISCOVERY.** The list is *what this profile
  can already reach*, which is a routing fact. How a reader learns a publisher exists **that this
  deployment does not already route to** is `EXTENSION-DISCOVERY`'s question and FEED §9.5's open
  navigation half; `publication_probe` answers *does this peer publish a feed* about a peer you
  already hold, and nothing invents one.
- **A TRAIT SHAPED BY ITS TEST DOUBLE RATHER THAN BY ITS PRODUCTION IMPLEMENTATION IS A SEAM THAT
  FITS NOTHING — `FeedSource`, found on the first day anything tried to use it (2026-09-10).**
  It was **synchronous**, and **every real source of a foreign entity in this repo is `async`** —
  `SignedSession::resolve`, `http_poll::fetch_*`, `foreign_cache::ensure_current`, without exception
  (one grep, and it was never run). That is not an inconvenience: the 2b gate bridged it with a
  spin-loop `block_on` over a noop waker, which is fine against `std::future::ready` and, against a
  `JsFuture`, **spins the main thread forever waiting on the event loop it is itself blocking.** The
  reader could never have been wired to anything.
  **What hid it is the interesting part: a `cfg` gate held for a GOOD reason, protecting a defect it
  was not about.** `feed_publish` is `cfg(not(wasm32))` because a *publisher* writes a directory and
  the browser has none — correct, and it also meant nothing that could expose the reader was able to
  import it. ***Ask what a cfg gate is keeping OUT, not only what it is keeping in.*** The seam built
  to make the reader testable had made it unshippable, and the gate that keeps the publisher honest
  is what kept the reader out of sight.
  **The repair copies `BinSource`'s spelling** — a boxed `!Send` future, no `Send` bound, owned key —
  because the production implementation of the new trait *wraps* that one and a second expression of
  *"an async source in this crate"* is C15's drift. **The friction is the proof:** a boxed future
  cannot borrow `self`, so the test's own origin now holds an `Rc` and an owned `PathBuf`, which is
  exactly what a `JsFuture`-backed implementation must do. The old trait let it hold borrows and
  `block_on` inside the method, and that compiled and passed. The new gate is
  `a_source_that_answers_later_is_still_a_source` — a source that returns `Pending` twice before it
  is ready, which is the property a synchronous trait **cannot express at all**.
  **And the render loop is the other half of the same fact: `render_dom` cannot await either.** So a
  window needs a poll-shaped adapter (`src/feed_fetch.rs`), and it is `HttpPollResolver`'s
  `Loading | Ready | Failed { retry_at_ms }` + `spawn_local` + repaint — the **third** expression of
  *a foreign artifact behind a synchronous surface*, and deliberately not a third design. The
  decision (`feed_step`, four outcomes) is **pure and takes the clock as an argument**, because the
  pump is `cfg(wasm32)` and `now_ms()` answers **0.0** natively — a decision reading its own clock is
  reachable only through Selenium. `Wait` is kept apart from `Serve` (conflating them renders an
  in-flight walk as *"this author has posted nothing"*) and `Retry` from `Start`.
  **A GREEN ANTI-VACUITY TEST WAS GREEN FOR THE WRONG REASON, AND ONLY MEASURING IT SAID SO.**
  *"An origin cannot serve one author's tree as another's"* passed — on a **404**, because the
  stranger's peer segment simply does not exist in that directory. A real property, and far weaker
  than the docstring's claim that the pin refused it. It now **performs the substitution** (copies
  the author's whole subtree under the stranger's peer id, so every path resolves and every hash
  checks out and the only thing wrong is *whose they are*), and it **asserts the reason** —
  `Verify("peer_id mismatch")`, with an explicit `!contains("404")` — because a regression that moved
  the paths would otherwise look identical to a working pin. ***If nothing you opened contradicted
  you, you did not run a check***, applied to a test's own passing.
  **Stated bound: nothing here is durable, so D24 does not engage yet** — and the analysis for when
  it does is in the module doc and is **not one answer for the whole feed**: the head and pages are
  fixed-key and **mutable** (currency owed), an entry is keyed by **its own** content hash and cannot
  go stale, and a signature is keyed by **the target's** hash and *looks* content-addressed while
  being mutable — a publisher can re-sign and the key does not move. That third row is the one a
  tidy implementation gets wrong.
- **THE GATE HELD THE CLOCK STILL, SO THE PROPERTY IT NAMED WAS NEVER MEASURED — the page-stamp
  defect, found reviewing 2b the same day it shipped (2026-09-10).** `posting_again_rewrites_only_the_last_page`
  asserts FEED §4.3 rule 1's whole [MUST] property — *"rewriting page 12 changes page 12's binding
  and nothing else — `O(tree depth)`, the same cost as posting"* — and it passed while
  `plan_index` stamped **every** page with the instant of the publish. Publish tomorrow and page 0's
  bytes move although it holds the same three entries it held yesterday: the archive is re-projected
  in full, every cached page a reader holds is invalidated, and rule 1's cost argument is defeated by
  **the ordinary act of posting** — the same sentence that module's own doc already used about the
  *ordering* half, one variable over. It passed because the test publishes twice at one `NOW`, which
  is **not a republish; it is one publish run twice.**
  ***Ask what your gate's expected value depends on.*** The charter already carries that rule in two
  shapes — a gate satisfied by its fallback, and a gate that DEPENDS on the defect — and this is a
  third: **a gate whose rig holds a variable still that the property is about.** The tell is cheap
  and general: *the test passes a constant where production passes a clock.* Grep your fixtures for
  a frozen `NOW` and ask, for each assertion, whether the thing being asserted is a function of it.
  **The fix is a WITNESS, not a prior-state read (AP44).** A page's `updated_at` is the newest
  `created_at` it carries — *this page last changed when its newest entry was written* — so an
  unchanged page reproduces its own stamp for free, forever, with nothing carried. The alternative,
  carrying the previous publish's stamp forward for untouched pages, keeps *publish time* as the
  meaning and holds **only as long as the publisher still has the out-dir that publish wrote**, which
  `C-2` measured is exactly what a second device does not have. `max`, not `last`: §4.5 makes the
  order **authored**, so a backdated post at the end of a page must not drag its stamp backwards. The
  **head** keeps the publish clock — it is one small mutable pointer that genuinely changes every
  publish, and it is what a reader polls.
  **The deciding fact is that §4.2 gives `updated_at` NO SEMANTICS — three CDDL lines in the whole
  convention and no prose** (exhaustive named search). So the reading is ours to make, the two
  readings differ **observably on the wire**, and one of them silently defeats a [MUST]'s stated
  property. Routed as `A-41`, implemented one way, pinned by a test at both levels (through the
  projection and in the pure planner) so a ruling moves one file. Falsified: a **one-line** neuter
  reds exactly those two and nothing else.
  **Second finding, same review, smaller: `read_one` reported a failed ENTRY fetch as
  `PageMissing { page: 0 }`** — rendering *"the head names page 0 and it does not resolve"* for a
  fault in an entry, pointing whoever read it at the index. **AP40's cost is not the collapsed value,
  it is the wrong sentence that comes out of it.** `EntryUnreachable { entry, detail }` now, and it
  stays apart from a short view (`Ok(None)`, the publisher withdrew the post) because those are
  opposite facts — shortening the feed on a fetch failure is D24's rule pointed at a reader, an
  outage rendered as posts someone deleted.
- **A RULE STATED IN A MODULE DOC IS NOT A RULE APPLIED TO EVERY INSTANCE IN THAT MODULE — the 2a
  review, 2026-09-10, and all three findings were in code written the day before by the same seat.**
  `make test` 1696 → 1699, `make crossimpl-site` **AGREED at the unchanged root**, `make lint` green:
  the site path did not move.
  **(1) `entity_ref` stated the collapse rule and left an instance.** The module doc's own sentence is
  *"we collapse both to absent in the ENCODER as well as the decoder, so the wire cannot express what
  the string cannot"* — it enumerated `via: []` and an empty anchor, and **`path: "/"` is the third**.
  `from_value` refused `""` and admitted `"/"`, which is the same address; `to_uri` rendered it
  `entity+ref://{peer}/`; `parse_uri` then correctly read that back as **`NoIdentityTerm`**, so a
  representable atom could not survive the round trip §3.2 makes a `[MUST]` in both directions. One
  predicate now (`is_addressable_path`), consulted by the decoder **and** the emitter, because the two
  ends have different wrong implementations. **Falsified — seen red, printing the atom that decoded.**
  ***When a module doc says "we do X to every Y", grep the module for Y*** — the doc is where a rule is
  most persuasive and least enforced, and this is `A-30b`'s shape (*we stated a rule and proposed
  breaking it in the same packet*) one layer in: the author of the rule is the one who misses the
  instance, because they are reading the sentence rather than the set.
  **(2) SECOND INSTANCE of the meaningful-default collapse, so it is no longer a one-off in the C9
  entry.** `IndexHead::oldest` used `uint(..).unwrap_or(0)`, and FEED §4.2 makes `0` a **real** default
  (*a publisher who has dropped nothing emits no key*) — so *absent* and *present-but-unreadable* were
  the same value, exactly as `min_rollback_index` was, in an unrelated subsystem two weeks apart. The
  direction is the costly one: an unreadable page floor read as 0 sends a reader walking back through
  pages the publisher already dropped while the head says otherwise. `optional_uint` splits them;
  absent stays a real zero. ***`unwrap_or(default)` on a decoded wire field is only safe when the
  default is not itself a fact*** — and if it is, the guard belongs in the decoder, not at the readers.
  **(3) AUTHORING AND TRANSCRIBING ARE DIFFERENT ACTS, and that is what decides where a producer bound
  goes.** EMBED §3's `inline-payload` is `bstr .size (1..16384)` and the range was enforced in exactly
  one place — `asset_store::stage` — while the union that *declares* it had moved to `embed.rs` as the
  shared home. So a FEED entry body could carry an oversized inline payload with nothing to say so
  (AP44: the rule lived at the importing carrier, not with the type).
  **The obvious fix would have reproduced the asset regression.** Enforcing on `EmbedPayload::from_value`
  looks equivalent and is not: the durable cache's write-through **re-encodes** a decoded foreign asset
  (`SiteAsset::to_entity`), 47% of a real published tree is over 16 KiB, and every one of those is
  *legacy*-encoded today — so a decode-side refusal would make each of them unreadable on the second
  visit, which is the 09-10 audit's defect with a new cause. D24 says it outright: a cache that drops
  what it cannot re-verify turns an outage into a missing figure — **and here we CAN verify it.**
  So: `EmbedPayload::inline_is_conformant` is the one expression, `stage` consults it, and the
  **decoder stays charitable on purpose.**
  ⚠ **Stated bound, measured rather than reasoned, and it is a real non-conformance:** a legacy
  oversized asset heals into the **declared shape** and a **size the CDDL excludes** — the cache
  write-through emits `inline` payloads over 16 KiB into the reader's own tree today. Inert (nothing
  but us reads that cache; the mirror is phase 4) and pinned by
  `a_legacy_oversized_asset_heals_into_a_shape_that_is_declared_and_a_size_that_is_not`, whose job is
  to be the **instrument for the concession, not its approval**. The repair is to re-`stage` through a
  content store, which the Worker arm has no content verb for — so it is a scoped piece of work, not a
  line to slip into a feed session. Do not describe the inline bound as enforced end-to-end.
- **AUTHORING A `default` POLICY ENTRY NARROWS THE REQUEST PATH FOR EVERY UNLISTED PEER — SHARE-10,
  and the withdrawal path is the same hazard at full strength (2026-09-10).**
  `ENTITY-CORE-PROTOCOL` §6.2 makes the **same** `default` entry a **union term** at §4.4's
  authenticate-response and a **per-peer ceiling** at `system/capability:request`. Both intended — and
  §6.2 also says that with **no** entry, request-time *"works by skipping the policy ceiling: step 3
  only enforces bounds that exist."* **So creating one where a deployment had none converts *no
  request-time ceiling* into *this request-time ceiling*.** The failure is silent in the direction that
  hides it: the publication becomes fetchable, so the change looks correct, while unrelated requests
  start failing subset-validation — the 13/13 → 7 P/6 F shape the kernel's own comment records,
  arriving **through the fix rather than through the ambiguity**.
  **We reached for this and stopped one step short.** We routed the ceiling/floor collision as *why
  §2.5 reads two ways*, which is true and is the smaller half; it is also **a live side effect of what
  we shipped**. *When you find that one key means two things on two paths, ask what your own write to
  it does on the path you were not thinking about.*
  **The withdrawal path is sharper than the vector as written.** `SHARE-10` is about *creating* an
  entry; `policy_writes_for` **empties** one when the last publication goes, and an empty grants array
  is a ceiling that permits **nothing**. That is correct for a named peer — where an empty entry *is*
  the revocation, and this module already had a test asserting it — and wrong for the fallback segment,
  where *revoked* and *no ceiling* are different states and **only one of them is expressible by
  writing**. `union_request_baseline` is the `[SHOULD]`: **if we write the fallback key at all, never
  narrower than what the deployment already intends to allow.** Fallback key only — unioning into a
  named peer's entry would *widen*, and the whole rule is about not narrowing.
  **Stated bound, pinned in a test rather than recorded in prose:** with no baseline the last
  withdrawal still writes an empty `default`. The correct end state is **no entry at all**, and
  `system/capability:configure` has no removal verb — a convention question, routed rather than decided.
  **Nothing supplies a baseline today and that is measured, not assumed:** `share::author_policy` is
  the only writer of any policy entry on our own peer (`backend_auth` writes to a *backend* peer, keyed
  by that peer's id, never `default`). **Do not describe SHARE-10 as discharged** — its vector needs
  enforcement on and a request-path assertion, and enforcement here is `debug_open_grants`.
- ⭐⭐ **AN IMPLEMENTED ENFORCEMENT CAN BE STRONGER THAN THE SENTENCE THAT SPECIFIES IT — so when a
  design re-reads a landed clause as a widenable default, go and read the CODE under the clause
  (2026-09-14).** `EXTENSION-SUBSTITUTE` §4's *"peer B serving peer A's content without A's signature
  is invalid"* reads like a posture, and a proposal priced departing from it as *"a one-clause
  clarification"* on sound reasoning about the bytes. It is implemented as a **signature-verified
  admission gate**: `verify_entry_signature_against(entry, source_peer_id, …)` demands a signature
  over the chain entry **by the content's publisher**, and `candidates_for` filters failures out at
  **enumeration time** — before the capability check, before any fetch. So the departure is three
  implementations, not a clause, and the capability dimension everyone was arguing about **cannot
  widen it, because it is not a capability check.** ⇒ ***a clause's cost is the cost of its
  enforcement point, and prose does not tell you where that is.***
  ⭐ **This is the exact mirror of the sibling lesson that had just been learned in the same document**
  — *an extension's summary of a core mechanism is not the mechanism* — and both are read-the-wrong-
  artifact errors, in opposite directions, four sections apart. **Symmetry is the thing to carry: a
  summary can be weaker than the mechanism, and an implementation can be stronger than the spec.**
  ⭐ **And the population lesson beside it, which is AP34/AP35's shape inside one file:** the chain had
  **seven** unit tests and **every one asserted that it does not run** (bare hash short-circuits · no
  cap · wrong handler · wrong operation · no resource · resource out of scope · empty chain). Seven
  refusal gates, zero candidate walks — so *"zero behavioural checks"* undersold it: the half that
  verifies, advances and ingests is untested at **any** level, while a green suite of refusals reads
  as coverage of the subject. ⇒ **when you inherit a mechanism, list what its tests ASSERT, not how
  many there are** — and a test named for today's short-circuit (`bare_hash_query_short_circuits`) is
  AP45: it makes the behaviour you are about to change read as a decision, which also makes it the
  cheapest falsifiable landing point for the change.
- **READING OUR OWN TRACKERS IS NOT READING THE CORPUS (2026-09-09/10).** A status answer assembled
  from `docs/status/TRACKER-*.md` reported *"`APP-CONVENTION-REFERENCE` landed, the blocker is gone"* as
  if it were news; it had been folded by arch two days earlier and is a row in their design register.
  **The trackers are our outbox** — what we sent and what came back — and they are structurally silent
  about everything the counterpart did that was not addressed to us. **Before answering "where are we"
  on anything cross-repo, read `entity-system-architecture`'s `docs/DESIGN-REGISTER.md`, the roadmaps and
  the tier standard, not our record of the conversation.** The register in particular is built for this:
  arch added it because *"the same conclusions were being re-derived from scratch, each time with a
  weaker argument than the original"*, and it gates that **every** design document is either cited by a
  row or marked as carrying no conclusion — **203 of 203, enforcing.** The tell that this went wrong is
  cheap to check: *a two-day-old landing reported as a discovery.*
  **AND ITS SIBLING, EARNED 2026-09-10: RECONCILE AGAINST THE COUNTERPART'S TRACKER, NOT A NAMED SEARCH
  OF THEIR TREE.** `entity-workbench-go/docs/status/TRACKER-entity-browser-rust.md` did not exist at
  `4f7e6a8`, which is where our tracker was last reconciled; it existed at `e44337b` twenty commits
  later, carrying **four asks pointed at us**. **Three were already answered in our tree and nobody had
  told them** — `B-5` (*"run our re-cut fixture through `DirFetcher::manifest()` and tell us where it
  stops"*) has been **green in `make test` since before they filed it**, walking their vendored `d940ce0`
  emission end to end. That is the `B-2c` delivery failure they had just named against their own board,
  in the opposite direction, one day later. **A named search finds packets; it does not find the file
  whose whole job is to say what they are waiting on** — and *"it did not exist last time we looked"* is
  not a reason not to look again. **Mirror their ids unrenumbered** so one fact cannot acquire two.
  ⭐ **THIRD FACE, 2026-09-17, AND IT IS THE ONE THAT COSTS A SEAT WORK: A FOLD IS NOT A DELIVERY, SO
  A ROW READING *"ROUTED, UNANSWERED"* IS A CLAIM ABOUT OUR INBOX AND NEVER ABOUT THEIR TREE.** Four
  of our asks (`A-67`–`A-70`) were **ruled and folded into four documents on the day we filed them**
  while our board carried them as awaiting an answer — and one of them said, in as many words, *"we
  hold the type UNWIRED and will publish no profile bytes while this is open."* It was not open. Arch
  named it against themselves as their own consumer-axis rule, and the half that is **ours** is the
  read direction: the outbox rule above says the tracker cannot see what they did that was not
  addressed to us, and *a ruling on our own ask* is the case where that is least intuitive, because
  the ask was addressed to them. ⇒ **when an ask is rowed as routed and you are about to act on its
  being open, `git log <their tip at filing>..HEAD` over the documents it names** — the same
  instrument as the published-corpus grep, pointed at a counterpart. *A held decision on a stale
  premise is more expensive than a late answer.*
- **WHEN THE CLAIM IS ABOUT WHAT YOU *EMIT*, READ WHAT YOU *PUBLISHED* — `A-35` withdrawn 2026-09-10,
  after being escalated in three consecutive packets on a premise nobody had measured.** `REFERENCE`
  §4's `[MUST NOT]` binds *"a producer that emits the atom at all"* against emitting `entity://` **in a
  link position**, and we told arch three times that *"we emit `entity://` in link positions today"*, so
  the next slice that published a feed would make us non-conformant and owe a corpus migration.
  **Measured: 0 occurrences across 364 published files** in `dist/` + `dist-site/` + `dist-federation/`,
  **including all 102 published `.bin` entity bodies.** The claim was a `grep` count over `src/` (~30
  hits) read as a conclusion, and the grep had collapsed **three families**: **dispatch URIs**, which
  §4's own first sentence says is what the scheme *is* (*"following a link is not dispatching to a
  handler"*); **one internal `navigate()` argument** that is never serialized; and the **static
  exporter, which rewrites every entity-native link form and has asserted
  `!demo_html.contains("entity://")` since long before the question existed**. The MUST NOT is
  **conjunctive** and we fail the second conjunct under every reading of the first. ⇒ **the publish verb
  is unblocked and no migration is owed.**
  **Two things to carry.** (1) ***A count is not a claim.*** A grep tells you a token appears; the
  clause governs a *position*, and no number of hits distinguishes them — enumerate the families before
  the total means anything. (2) **This is the THIRD blocker in four days to dissolve on measurement, and
  all three failed toward *more* coordination**: `G-PIN-4`'s placement/keypair/peer-id trio (answered by
  one sentence of `EXTENSION-TREE` §3.2), FEED's `body: embed-node` (a production defined *nowhere*),
  and this. The charter already says *a blocker that asks for more coordination is the one to re-read
  the spec about*; **the cheap instrument this adds is the published corpus** — one `grep` over `dist-*`
  costs four minutes and would have caught it on day one. **Unwithdrawn:** `A-35`'s EMBED §3 half (our
  inline directive lowers to no `Embed` entity) is untouched, and **no gate can see it**, because a
  *missing* emission is not a divergent tag.
  **⭐ THE GENERAL FORM, EARNED THREE TIMES IN ONE SESSION AND AUDITED THE SAME DAY
  (`AUDIT-THE-PUBLISHING-PIPELINE-AND-WHY-ITS-MODEL-IS-UNFINDABLE-2026-09-10.md`): NAME THE ARTIFACT
  YOUR EVIDENCE CAME FROM, IN THE SENTENCE THAT MAKES THE CLAIM.** A grep count over `src/` reported as
  a fact about the **published corpus**; a mental model reported as **`resolve_publish_source`**; a
  build-time default (`ENTITY_STARTUP_SURFACE` = `chrome`) reported as the **publish flag**
  (`--surface` = `window`, and the doc was right) — *the third one inside the audit of the first two,
  in the section cataloguing other people's drift.* **The instrument was fine every time; what it
  measured was misidentified.** *"0 in `src/`"* and *"0 in the published corpus"* are different claims.
  **AP56 — a fact filed under the exception is not filed under the rule.** `AGENTS.md` **already
  stated the publishing model**, correctly, at line 1481, since **2026-09-04** — in `a9bc377`, whose
  subject is *"withdraw the invented blocker — a site and a feed are one mechanism."* **The entry that
  exists because a feed blocker was invented and withdrawn contained the answer, and a different feed
  blocker was invented six days later with it in context.** It is unfindable because it sits mid-entry
  under a heading about something else, framed as a qualifier to *"the one genuinely absent verb"* — a
  statement of a **limitation**, not of the **rule**. ***When you write down a limitation, ask whether
  you have just written the only statement of the rule it limits.***
  **And the corollary with a number on it: the fix for *"the model was not where I looked"* is almost
  never *"state it again where I looked."*** On finding it absent from where I searched I added **28
  lines** restating it — to a **3574-line** file that already held it, against `AGENTS-STANDARD`'s
  *"keep it short … names the trigger and does not carry the content."* **The charter accumulates
  restatements of facts it already holds, and each one makes the next lookup harder.**
  ⇒ **Before adding a publishing fact here, `grep` `REFERENCE-PUBLISHING-PIPELINE.md` for it — and if
  that file is wrong, fix that file instead.** It leads with the correct model and, measured
  2026-09-10, is **17 days stale and records two SHIPPED capabilities as deferred** (static-export
  images ship; the registry name layer ships), which is the expensive direction: *a shipped capability
  recorded as deferred reads as work still owed.* Audit §9 has the sequenced repair; it is not landed.
- **A FIX HAS TWO AXES — THE ONE IT WAS WRITTEN FOR AND THE ONE IT MOVES. RUN THE GATE THAT
  MEASURES THE COST, NOT ONLY THE GATE THAT MEASURES THE CURE (2026-09-09, found in a release
  audit).** The 09-08 connectivity arc's whole purpose is *fail now rather than at the 30 s
  deadline*, and it shipped with its own new gate (`e2e-webrtc-vanish`) proving the cure. **Failing
  fast costs a re-establishment**, and the gate that measures establishment cost —
  `e2e-webrtc-meet`'s §11.5 single-flight assertion — was not in the run set. Measured three weeks
  later, app tree byte-identical on every arm, fresh browser pair per run: `e17d711` **4,4,4** →
  `0f858df` **4,4,8** → `9e0cbd5` **8,8,8** → `e6213f1` **16,8,12,8** (bound 8). **Monotone
  accumulation across three commits, not one culprit** — each makes a dead-or-superseded connection
  fail sooner and each costs one more negotiation.
  **Single-flight is intact and saying so precisely is the difference between a bug report and a
  panic:** the node log shows a repeating **quartet** (`906/517/528/463` bytes), so one negotiation
  is still the conformant ~4 deposits — the scenario now runs *two to four* negotiations where it
  ran one. §11.5's bound is arch-pinned *per establishment* while the harness counts per
  `(caller, key)` **per run**, so read a red as *establishments × 4* before reading it as brute
  force.
  **Three method points, and the middle one is why this hid for three weeks.**
  (1) **`CORE_RUST_REF` turns "the kernel moved" into a bisect** — this was its first real use, and
  a four-arm single-variable A/B across a sibling repo cost four `make wasm` builds and no writes to
  their tree. Reach for it the moment a red is suspected upstream.
  (2) **A gate absent from the recorded run set is a gate nobody ran.** The last recorded `meet`
  deposit figure was **2026-08-19** (4/side); every 09-0x handoff records test · test-tauri · lint ·
  e2e-worker · wasm · site-dist and **no WebRTC gate at all**. The suite did not regress silently —
  it was not consulted. **When you list a gate run, list what you did NOT run.**
  (3) **A variable metric needs N runs per arm.** At `e6213f1` it reads 8, 12 or 16; one run proves
  nothing in *either* direction, and a single green would have closed this. Different cause from the
  stale-grid lesson below — there the *rig* drifted, here the *product* is nondeterministic — same
  discipline.
  **And what was deliberately not concluded:** whether each extra negotiation is a legitimate
  recovery or a spurious teardown. The node vantage cannot see it, the ~8.8 s gap on a link that had
  just carried a message is suggestive, and suggestive is not measured — a lead was routed *as a
  lead*. Packet:
  `docs/status/ROUTING-2026-09-09-c-kernel-THE-VANISHED-PEER-ARC-COSTS-EXTRA-ESTABLISHMENTS.md`.
- **A MEASUREMENT YOU ROUTE CAN GO STALE IN THE OTHER REPO — and once it is folded into someone
  else's normative text it reads as current (2026-09-09).** We told arch on 2026-08-16 that *"we do
  not run a §5 keepalive on the WebRTC path"*. True when written. `git -S spawn_keepalive` over
  `core/peer/src/remote.rs` returns **one** commit — `d8ef14d`, **2026-08-23**, Amendment 12 — which
  added it to the tail §10.3 traversal connections share with ordinary dials. The claim was false a
  week later, and by then it was quoted inside `EXTENSION-NETWORK` §10.3 obligation 2 as the
  empirical grounding for a MUST. **A fact about the kernel that you measured is a fact with an
  expiry date; a fact about the kernel that you PUBLISHED is one somebody else is now reasoning
  from.** So: when you route a measurement of a sibling repo, name the commit you measured it at
  (`(symbol, path, commit)` is already the rule and it is exactly what would have dated this), and
  when the sibling moves under you — the `git -C ../entity-core-rust log` reflex this charter already
  teaches for red tests — **re-read what you have told other people, not only what your tests say.**
  **The second half is where the value was:** reading the tail to check our own stale claim found
  that the **§7.4.1 Responder runs no §5 keepalive at all** (`adopt_transport_connection` early-returns
  into `serve_traversed_connection`, *above* the spawn site; three `spawn_keepalive` call sites, all
  in `remote.rs`), and that the role — hence which browser is unprotected — **flips run to run** with
  peer-id ordering. Nobody decided it: `6558a68`, the §7.4.1 role split, is the **immediate
  descendant** of the commit that added the spawn. Same shape as *a teardown that evicts its own
  binding disarms the §A1 demotion* — a correct change at one layer carving a MUST out of another.
  **And obligation 2 contains a MUST we had never discharged:** *know and declare which mechanism
  holds your mapping open.* Declared now (we rely on the **substrate** — RFC 7675 consent freshness,
  both sides by construction; §5 covers one side as a by-product; the chat poll is not a mechanism we
  intend to keep) in `BUILDOUT-SIGNALING-AND-NETWORK-EXTENSIONS.md` §2.3. **A MUST phrased as
  "declare X" is invisible to every gate you own** — nothing goes red for silence, which is precisely
  why the clause says silent reliance *is choosing nothing*. Packet:
  `ROUTING-2026-09-09-b-arch-THE-EVIDENCE-IN-10-3-OBLIGATION-2-WENT-STALE.md`; the A3 investigation
  that found it: `docs/plans/PLAN-2026-09-09-b-RETIRING-THE-CHAT-POLL-WHAT-IT-ACTUALLY-COSTS.md`
  (recommendation: **do not retire the poll before the release** — four prerequisites, two in the
  kernel, and on the **Worker arm the poll is the whole delivery mechanism**, which no prior account
  of that item had priced).
  ⭐ **SECOND INSTANCE 2026-09-14, AND IT RUNS THE OTHER WAY: THE STALE MEASUREMENT WAS OF *OUR* TREE,
  TAKEN BY *THEM*.** Arch's `EXPLORATION-THE-VERIFICATION-LADDER` §2 carries its own self-described
  *"single most important line"* — ***nothing signs individual entities today*** — measured at our
  `d9cc645` (2026-09-06). `publish_feed` has minted a `system/signature` per `app/feed/entry` since
  **`faf73cb`, 2026-09-10**, so their composer-contract row reading **"RULED, unlanded"** was four
  days stale, and it is the row a reader prices the whole design off. **Nobody did anything wrong** —
  they measured correctly and we shipped after. ⇒ the rule above said *re-read what you have told
  other people*; **the addition is that a counterpart measuring YOU has the same expiry problem and
  no way to know**, because our trackers are an outbox and are structurally silent about what we
  shipped since we last wrote. **When you land a capability a counterpart's open design reasons
  about, the delivery is telling them — a landed feature nobody was told about is indistinguishable
  from an unlanded one, and strictly worse, because it makes their table look checked.** Cheap
  instrument, same shape as the published-corpus grep one entry over: when a counterpart's document
  cites one of our commits, `git log <that commit>..HEAD` over the subsystem it is about.
- **A DURABLE IDENTITY IS NOT A DURABLE SEQUENCE — the boundary is what a PROCESS carries, not
  what it writes (C-2, measured 2026-09-09).** Signed-root continuity is recovered from the
  **output directory** (`RootProjector::adopt_prior_head`), because every CLI run builds a fresh
  in-memory peer and only the *keypair* is durable. So two out-dirs under one key are two
  independent sequences — arch's multi-device-publishing case, reachable with no second machine.
  **The headline: a session that read one device refuses the other with
  `Verify("seq rollback: cached 1, received 0")`** — the publisher's own second machine,
  indistinguishable from an attack. So the consumer's only defence against a rollback is also what
  breaks legitimate multi-device publishing, and a fix cannot be *"tighten the floor"*.
  **The cell with no defence is EQUAL seq, not lower:** two devices at one `seq` with different
  content are both accepted and nothing in the chain has anything to say — the same shape as *two
  trees both at zero never go backwards*, and it is exactly where *"give the second device a
  starting sequence"* lands every deployment. Read `multi_device_sequence.rs` before proposing that.
  **The falsifier is where the cause actually is, and it is the transferable move: the rig varied
  TWO things at once.** One `RootProjector` held across the *same two out-dirs* advances `0 → 1` and
  is accepted in either order — so directories are not the cause, **processes** are. **When a rig
  changes two variables together, hold one and re-run before you name the cause**; a measurement
  that names the wrong variable produces a fix that relocates the defect. **Stated bound:** Tori
  cannot publish (no verb reads a long-lived native store), so two out-dirs under one keypair is the
  honest stand-in — say which one you ran. The fix is `core/peer`'s; ours is the measurement.
  Packet: `docs/status/ROUTING-2026-09-09-arch-C-2-MEASURED-THE-SECOND-DEVICE-IS-A-ROLLBACK.md`.
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
  **AND THE HARD STOP HAS A DOOR BESIDE IT: `builds.json` is the ORIGIN's history, and
  `make site-dist` builds into a FRESH directory (2026-09-09).** An *absent* manifest is a first
  publish — `next_index()` 0, empty list — which is right, and cannot be refused without breaking
  the actual first publish, so the malformed-manifest hard stop above does not cover it. Measured:
  `entitychurch.org` named **two** builds with the live one at **index 1**, while a same-day
  `make site-dist` produced a manifest naming **one** at **index 0**. Uploading that un-names both
  retained shells and walks the counter backwards, on the deploy where a fallback is worth most.
  It fails in the **safe** direction (unnamed ≠ 404), so the loss is the *capability*, not an
  incident — which is exactly why nobody notices. **Seed `<dir>/builds.json` from the origin before
  running `builds`**; continuity is the uploader's to carry, not the build's.
  `PUBLISHING-QUICKSTART` §6.2a is canonical for it.
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
  live on a real domain. It also serves `/sw-selfdestruct.js` **200**, so C17 is
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
  **THREE FRAMING ERRORS IN THE FIRST WRITE-UP OF THIS MEASUREMENT, corrected by meta-devops on
  2026-09-09, and the class is worth more than any of them: WE RE-DERIVED ESTATE FACTS AND REPORTED
  THEM AS DISCOVERIES.** The numbers all matched theirs — which is the useful half, an independent
  check — and the framing was wrong three times. **(1)** *"`10a5398` was in no handoff"* is **false**:
  it is in **six** places in this tree, including `RELEASE-2026-09-05-…`, *our own release document
  from four days earlier*, and in their deploy runbook. A named search of `docs/status/` for a commit
  label costs one grep and was not run. **(2)** *"row 10 real for the first time"* stated a
  deployment fact as a first sighting; devops had recorded entitychurch's two slots on 09-06 in a
  runbook **this repo does not have a copy of**. The fact is right; *"for the first time"* was a
  claim about the record, and it was not our record to make. **(3)** *"`AGENTS.md` said the fleet has
  no slots"* was **correct about THIS repo's charter** (verified at `7b80161`, line 1652) and read as
  a claim about *theirs*, whose `AGENTS.md` contains the word *slot* zero times. **In anything that
  crosses a repo boundary, `AGENTS.md` is ambiguous by construction** — there is one per repo plus the
  injected standard. **Say *this repo's charter* or name the repo.**
  **The rule: the estate is not ours to state.** Before reporting a deployment fact as new, grep our
  own `docs/` for it and assume the seat that owns the estate already knows. An independent
  measurement that agrees is worth reporting *as a check*; the same measurement wearing a discovery's
  clothes spends credibility for nothing.
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
  not a lockfile and does not choose a ref for you. **Their pipeline's construction is still
  meta-devops': `estate.conf` declares the pair and `publish.sh` builds from git worktrees at the
  declared refs. Our guard runs INSIDE their worktree and must pass trivially — a real assertion,
  not a second opinion. Do not build THEIR publishing pipeline here; this repo makes the contract.**
  **What WE build is the local half, and as of 2026-09-09 `CORE_RUST_REF` CONSTRUCTS — see the next
  entry. The line that used to sit here, "do not build worktree construction here", is retired: it
  was right about their pipeline and was read as a ban on pinning our own builds at all.**
  **AND THE FIRST CUT GUARDED THE PATH *WE* RUN — meta-devops found it in a day (2026-09-09).**
  It went on `make site-dist`, our release target. **Their production pipeline calls `make site`**
  (`publish.sh:384`, `make -C $BROWSER_RUST site …`), which had no guard at all — so the commit
  message correctly named "the site publish is built locally, where nothing pinned anything, which
  is the one path that reaches a deployer" and then guarded a different path. *The path that reaches
  the deployer was not ours to see, and that is exactly why the guard must be structural rather than
  placed.* Now **one macro, `$(check_build_pair)`, on all six verbs that hand bytes to a third
  party** — `site` · `site-dist` · `site-bare` · `registry` · `dist` · `dist-web` — with `federation`
  (a local rig) and `builds-manifest` (reads an already-built shell whose pair is already stamped)
  **deliberately excluded and named as such in the Makefile**, so the next reader does not re-derive
  the set. **When you add a guard, enumerate every entry point into the thing you are guarding, not
  the one in front of you** — and if another seat invokes this repo, ask which target they call.
  **Quote build ids as `(our commit, entity-core-rust commit)` in anything a deployer reads.**
- **`CORE_RUST_REF` NOW BUILDS THE COMMIT IT NAMES — construction, not verification, and THE PIN
  POINT IS THE CONTAINER MOUNT (2026-09-09, `tools/core-pin.sh`).** The guard above made the pair
  observable and refusable. It could not help in the one case that actually bites on this box: **we
  share a machine, and the other seat is often mid-edit in `entity-core-rust` right now.**
  Verification can only tell you to come back later — and worse, **`--check` runs BEFORE the build**,
  so a commit landing mid-`make wasm` produces a mixed artifact behind a green guard. *A check that
  runs before the window cannot close the window.*
  **Every containerized verb already mounts the parent at `/src/entity-systems`, so a SECOND `-v`
  over `/src/entity-systems/entity-core-rust` overlays it** — verified before it was designed on
  (`-v parent:/src -v pin:/src/sib` → the pin's bytes). All **thirty-three** path deps (25 here, 8
  in `src-tauri`) resolve through that one path, so pinning costs **zero `Cargo.toml` edits, zero
  symlinks, and zero writes to the sibling's git**. Cargo is never told anything; it resolves the
  paths it always did and finds different bytes.
  **`git archive`, deliberately NOT `git worktree add`.** A worktree registers itself in the
  *sibling's* `.git/worktrees/` — a write to a repo that is not ours, which AGENTS-STANDARD forbids —
  and it leaks rows into the other seat's `git worktree list`. Not theoretical: `entity-core-rust`
  carries two stale `/tmp` worktrees marked `prunable` today. The export lands in
  `~/.cache/entity-browser-core-pin/<sha>/` (**outside the tree** — it was `.core-pin/` inside it until
  2026-09-15, and `vocab-lint`'s analyzer read the kernel's `*.rs` there as ours and reported
  `single-seat app/user`; gitignore hides a directory from git, not from a tool that walks the tree), keyed by **resolved commit** so reuse can never be wrong, extracted through a
  scratch dir and `mv`'d so a pin is complete or absent. ~13 MB, 57 ms; `make core-pin-clean`.
  **Unset stays the default and the default is unchanged — the live sibling.** Always-live means
  always-latest, which is what you want while two seats iterate; a pin you must remember to move is a
  pin that goes stale. Pinning is **per-invocation**, so neither mode is a standing commitment.
  **`NATIVE=1` REFUSES a pin** rather than ignoring it — no container, no mount, so the build would
  read the live checkout while the operator believed it was pinned. A guard that silently does not
  apply reads as covered (AP36).
  **Three things it cost to get right, and none was visible from the design.**
  **(1) A pinned build needs its OWN `CARGO_TARGET_DIR`, and this is correctness, not hygiene.**
  `git archive` stamps extracted files with the **commit's** date, so pinning to an older commit
  yields sources *older* than artifacts already in `target/` from a live build — and cargo
  fingerprints path deps by mtime, so it would call them fresh and relink the artifact built from
  the **live** tree, silently serving exactly the bytes the pin exists to exclude. Measured: the
  export of `e6213f1` carries mtime 14:04 (its commit time), the live working copy 13:59. The cheap
  fix is wrong in the mirror: `touch`ing the export to *now* makes pin→live wrong instead. Only
  separate target dirs make the two independent of each other's clocks. `CARGO_TARGET` is
  **recursive (`=`)** so per-target and recursive-make `TARGET_DIR` overrides each get their own.
  **(2) `build-stamp.sh` runs INSIDE the container and reads the pair through `build-pair.sh`**, so
  without `ENTITY_CORE_PIN` carried in, a pinned release would stamp weaker provenance than an
  unpinned one — the strongest build we can make describing itself as the one we could not identify.
  **(3) `git` WALKS UP, so "not a checkout" reported a NEIGHBOUR'S COMMIT, not `unknown`** — a
  latent defect in `build-pair.sh` predating this work and the exact outcome its own doc comment
  promises is impossible. `git -C <export> rev-parse HEAD` finds no `.git` and searches ancestors;
  **`<shared-parent>` is itself a repo**, so the answer was its `0bc11b3`. Provenance naming the
  wrong repository's commit is worse than provenance admitting ignorance — nothing downstream can
  tell them apart and the number looks plausible. `read_ref` now confirms `--show-toplevel` **is**
  the directory asked about (`pwd -P` both sides). **Found by falsifying the env pass and reading
  the value instead of the pass/fail** — the neuter did not produce the failure predicted for it,
  and the difference was the finding.
  **Falsified, not asserted:** pinned to the commit *before* `reader_ended` landed, the container
  reads **0** occurrences at the path Cargo compiles while the live tree reads **11**; the stamp
  reports `unknown` with the env pass removed and `e6213f1` with it; `--check` says *pinned by
  construction* and skips the on-disk comparison, because **construction outranks verification** —
  re-running it would refuse a build strictly more reproducible than any this check could pass.
  **What this does NOT do:** it does not choose a ref for you, and it is still not a cross-repo
  lockfile. A clean-but-unintended ref still builds. It makes the pair *choosable*; deciding **which**
  pair is a release is still a human act.
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
- ⛔⭐ **AND THE SERVE PORT IS THE WORSE HALF OF THE SAME COLLISION — `:8092`, 2026-09-16.** The entry
  below is about the **grid**; this is about the test's own HTTP server, and the two fail in opposite
  ways. A grid collision **refuses** (`webdriver server did not respond`), which reads as a rig fault.
  **A serve collision ANSWERS — with another seat's build** — which reads as a product defect.
  Measured: 79/3 unfiltered, and two of the three were this.
  `a_chat_window_can_be_pointed_at_a_second_peer_without_reopening_it` died on
  `connectionFailure` to `localhost:8092`, and
  `a_default_profile_can_post_to_its_own_tree_and_the_post_survives_a_reload` reported
  **`RED (VACUOUS) — the Feed window body rendered nothing`** over a window whose copy
  (*"Follow a publisher by peer id"*) **does not exist anywhere in this tree** — because it had loaded
  the VM worktree's app, measured separately at `:8092` as `55eca78a-dirty`. On `E2E_HTTP_PORT=8490`
  both pass, verified one at a time.
  ⇒ ***the cheap tell is a needle that cannot be true: grep the failing assertion's own dumped text
  for a string in your source, and if it is not there you are looking at somebody else's build.***
  A vacuity guard firing on a surface you just watched work by hand is the same signal from the other
  end. **`E2E_HTTP_PORT` is the knob and `make e2e-worker` does not pick a free port** — `ss -ltn`
  first; this box carried **119 listeners** the day this was measured, and a port free an hour
  earlier was not.
  ⚠ **Related and separate: `make serve PORT=` fails the same way and louder.** `8201` looked free,
  was taken, and the serve died with `Address already in use` in a backgrounded log **while an
  unrelated process answered `200` on the apex** — so the first probe read *"apex healthy, deployment
  config 404"* and pointed at the publish. Check the log of a backgrounded serve, and verify the
  build stamp of what answered.
- ⛔ **A 55-OF-81 RED CAN BE A PORT COLLISION. `make e2e-grid` IS `--network host`, SO IS THE OTHER
  SEAT'S SELENIUM, AND BOTH WANT :4444 (2026-09-16).** Measured: `tcabs-fx` (another seat, same
  image) had held :4444 for two hours; `make e2e-grid` stood ours up beside it and the suite read
  **26/55**, then **25/56** on a second fresh-grid headless run — reproducible, which is exactly what
  makes it read as a catastrophic regression rather than a rig fault. The tell is in the failure
  text, not the count: `webdriver server did not respond` and `Connection refused` on
  `127.0.0.1:4444` for everything after the first few tests. **The fix is one flag and the Makefile
  already documents it** (`WEBDRIVER ?= a PRIVATE grid. This box routinely carries other seats'
  Selenium`): `make e2e-grid GRID_PORT=4455` then `make e2e-worker WEBDRIVER=http://localhost:4455`
  — which took the same tree to **80/1**. `GRID_NAME` derives from the port, so the private grid is a
  separate container and **neither seat touches the other's** (still: never kill theirs — take
  *yours* down with `make e2e-grid-down`, which is what stops the interference).
  ⇒ **Before believing any unfiltered red, `podman ps` for a second `standalone-firefox`.** Two
  seconds, and it separates *the product broke* from *two containers want one port*. Same family as
  the stale-grid A/B lesson — **the rig is shared mutable state and the other seat is part of it** —
  and note this is a *different* mechanism from the retired SELinux advice: that one was a relabel
  war with no clean escape, this one has a documented flag.
  **And the residual after the fix was not ours either.**
  `a_drag_on_a_windows_grip_survives_the_window_rebuilding_under_it` reds under concurrent load
  (`swaps: 1`) and is **4/4 green alone with `swaps: 0`**. Its own comment names the mechanism —
  *"the monitor rebuilds every second, so one can land between this script and the press"* — a 1 Hz
  rebuild racing a 2.5 s hold, on a box also running another seat's `rtc-a`/`rtc-b`. **Settled
  structurally rather than by an A/B:** the test opens the **System Monitor**, nothing anywhere
  compares `ResolvedPage` for a rebuild, and the diff touched no window chrome — *ask whether your
  change can REACH the failing surface before you spend half an hour bisecting it.*
  ⚠ **CORRECTION 2026-09-17: "concurrent load" is NOT what the numbers say, and "4/4 green alone" is
  true only of a CLEAN `dist/`.** Measured at `ed2cec40` on a private grid, unfiltered **87/1** with
  this as the only red. It reds **in isolation too** — 6.65 s, nothing else on the grid — so load is
  not the differentiator. What tracks it is whether `dist/` carries fixture publish output: **clean
  dist 0 failures in 4 runs; polluted dist 2 failures in 3**, at the same load average (~4.9–5.1).
  ⇒ **the suite pollutes its own rig**: `emit_deployment_config_fixture` and its siblings default
  their out-dir to `"dist"` (`content_site/publish.rs`), so by the time this gate runs in an
  unfiltered pass, `dist/` is serving a peer tree + `sites/` + `content/` + `transport-profile`, and
  every later test boots against it. `SKIP_BUILD=1` is what makes that persist across runs; a bare
  `make e2e-worker` rebuilds `dist/` and washes it out — which is why the gate looks clean when you
  re-run it the obvious way, and why the isolation that "proves" it was load is the isolation that
  also cleaned the rig. **Both arms are intermittent, so neither number is a threshold** — the
  mechanism is NOT established (a populated tree plausibly costs extra boot work and extra window
  rebuilds, but nothing here measured that). ⇒ ***when a gate is green alone and red in the suite,
  check what the suite WROTE, not only what it was running*** — a shared out-dir is rig state the
  same way a stale grid is, and it survives the run.
  ⚠⚠ **THIRD CORRECTION, 2026-09-19 at `811d415a`, AND IT RETIRES BOTH PRIOR CAUSES: THE GATE REDS ON
  THE WRONG ONE OF ITS OWN TWO ASSERTIONS.** Measured with `dist/` verified clean (no `sites/`, no
  `content/`), a private grid, load 7.45 on 32 cores: **3 failures in 18 runs**, and no named
  variable tracks it — fresh container (3 pass / 1 fail), position in the session (it failed on
  session **2**, not 1), dist pollution (clean throughout), build freshness, and load.
  `HOLD_STALE_MS` is **30 s** against a 4.3 s drag, so the staleness arm is excluded too.
  ⚠ **Read that rate as "it is intermittent", not as a number.** Its sibling
  `an_app_republished_…_on_the_worker_arm` was measured the same afternoon and its failures turned
  out to be **clustered** (`ppppFFFF` at n=8), which makes any small window read as deterministic —
  see the GOTCHAS entry, where exactly that produced a wrong, committed determinant. 18 runs of a
  clustered process bounds the rate loosely and settles nothing about a cause. **The mechanism below
  is a direct observation and does not rest on the rate.**
  ⭐ **What settled it was timestamping the swaps** (a probe in the test's own `MutationObserver`, run
  and reverted). On a failing run: `press_at 1381` · held probe `t=3999`, `swaps 0`, `swap_times []`
  · release probe `t=5895`, **`swap_times [5893]`**. The one swap is **2 ms before the probe read**,
  ~4.5 s after the press — *outside* the 2.5 s hold and *outside* the 4.0–5.8 s move window. So no
  mid-drag rebuild occurred, and `after["swaps"] == 0` is read immediately after
  `perform_actions(…Up)` returns, where the release handler legitimately clears `grip-drag`, calls
  `set_held(None)` and repaints — **replacing the grip is what a release is supposed to do.** That
  assertion is structurally racy against its own teardown.
  ⇒ **The real symptom is the NEXT assertion, which never runs.** In that same run the drag did not
  land: height **617** against `h0` 619 and a wanted 494, grip 640 → **642.7**. Line 21404 (`swaps`)
  fires before line 21407 (height), so every session that has looked at this gate has been reading a
  message about *"a rebuild mid-drag"* for a run in which there was no rebuild mid-drag. ***When a
  gate has two assertions and the first one is about a precondition, check which one is actually
  failing before you believe its sentence*** — three sessions have now diagnosed the environment
  because the louder assertion named it.
  ⛔ **Cause still OPEN; the hypothesis is cheap and is NOT established.** `pointermove`'s own
  *"no button down means the release happened where we did not hear it"* branch
  (`p.buttons() & 1 == 0`, `build_size_grip`) would produce exactly this triple if a synthesized
  WebDriver move ever reports `buttons = 0`: the drag ends at the first move so no move is applied
  (height stays ~614), the shield comes down, and the resulting `rp()` lands its rebuild around
  release time. Testable by probing `shield` *during* the move rather than only at the ends, which
  means splitting the single blocking `perform_actions` call. Not attempted here — a subtle
  input-synthesis question at the end of a session in another subsystem is the thing `76a8e960`'s
  own note refuses.
- ⚠ **AND THE RUN BEFORE THOSE WAS INVALID BY MY OWN HAND: I falsified a lint WHILE the suite was in
  flight**, editing `src/content_site/resolver.rs` four times (two neuters, two `git checkout`s).
  This file already says *"do not edit `src/` while the suite is running — fixtures shell out to
  `cargo test` AT RUNTIME"*, and I read that rule as being about *feature work*, not about the
  two-minute neuter-and-revert cycle that is the most likely thing to overlap a long background run.
  ⇒ **a falsification is an edit to `src/`.** Finish every edit, commit, *then* start the suite —
  and if you have already started one, the neuter waits.
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
  ⭐ **A FOURTH CAUSE, 2026-09-16, AND IT IS NOT THE BOX: A FIXED SETTLE IS A THRESHOLD NOBODY
  EARNED.** A load excuse was written into a handoff — two runaway `make` processes from another
  seat — and it did not survive being checked: **load average 3.2 on 32 cores**, and the same
  invocation on a fresh grid came back **82/1** against the recorded 83/0, with one specific,
  nameable failure instead of a moving set. ⇒ **"the box is busy" is a measurement, not an
  adjective; take the load average before you write it down.**
  The one real failure was `worker_boots_and_opens_all_windows`, and the diagnosis was in the
  failure's own printout: the `put` scrollback it echoed was **the output of `help`**, so 800 ms
  after dispatching Enter the shell had not echoed the command, let alone run it. `shell_submit`
  dispatches and then `sleep`s a flat `settle_ms` — the shape this file already forbids for
  `poll_json`, at 74 call sites. **Raising the number buys a quieter box and loses the next one**;
  `shell_poll` re-issues the command until its own output satisfies a predicate or a deadline, which
  is strictly stronger, since a mirror that never fills still reds. It **re-submits rather than
  re-reads**: an `ls` renders its answer once, so re-reading its scrollback polls a photograph.
  ⛔ **And the vacuity under it, which is the half worth carrying: `last_shell_output` falls back to
  the WHOLE scrollback when the command never echoed — and the needle was in the part the fallback
  added.** The assertion is that `ls app/e2e_deltest` contains `marker`, and the
  `put app/e2e_deltest/marker marker …` echo three lines up contains it too. So a shell that never
  ran the listing returned a haystack in which the needle was **guaranteed**, and the create half of
  a delete-reflect gate could pass for the exact reason it should fail. ⇒ ***when a fallback widens
  the haystack, ask whether the needle lives in the part it added*** — and keep *absent* apart from
  *empty* (`last_shell_output_strict`) wherever PRESENCE is the claim. The absence half keeps the
  lenient one on purpose: there the same fallback fails safe.
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
- `file_kinds.rs` + `user_files.rs` + `kept_files.rs` — the File Manager's model. **Its *private* vs *shared* is a
  second permission system (AP57)** — three types with one body, three content namespaces, sharing by copy — and is
  being unwound onto grants (`BACKLOG.md` B-9, `AUDIT-2026-09-14-d-…`). Do not add a type, a namespace or a prefix to
  decide who may read a file.
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

## Routing packets and the per-counterpart trackers

**One tracker per seat we exchange work with, at `docs/status/TRACKER-<their-repo>.md`.** Seven
exist: arch, `entity-core-{rust,go,py}`, `entity-workbench-go`, `<devops-tree>`,
`entity-core-papers`, `entity-apps`. **They are the delivery** — arch reconciles against the tracker,
not against a directory listing, so an ask that is open and unanswered needs no re-send. Four
sections (Open — asks · Corrections we owe them · Filed, nothing owed back to us · Closed), **stable
ids that are never renumbered**, one sentence per ask naming what must be *decided*.
Per `SEAT-CLEANUP-INSTRUCTIONS-2026-09-09` (arch's tree).

Four rules that carry the weight:
- **"Filed, nothing owed back" is a real section, and most documents belong there.** Treating a
  for-information review as an open ask is the specific error that made arch report 44 open items
  against a seat whose true number was eleven.
- ***Filed* ≠ *routed* ≠ *answered*. Default to not established** — and evidence delivery by a named
  search of *their* tree for the packet stem or its subject, never by our own filename existing.
- **Archived is not delivered.** Close on their receipt or their reply, never on our own work
  finishing. (`entity-workbench-go`'s rule, and it is the best sentence anyone has written about
  this channel.)
- **An implementation defect in a spec that is already clear is not arch's** — it goes on the
  implementing seat's tracker.
- ⭐ **A packet marked *NOT TO BE SENT* is indistinguishable from one nobody wrote — *"the operator
  will decide it directly"* is a reason not to ASK, never a reason not to TELL (2026-09-15).** The
  whole entity-apps VM handoff was designed, written in full as
  `ROUTING-2026-09-13-a-entity-apps-…`, and held on `D5`'s reasoning that the operator runs both
  seats. Defensible for the *asks*; the consequence was that for two days a complete handoff existed
  undelivered with **nothing anywhere saying it was held rather than done**, and this seat could not
  answer *"what is the status with entity-apps"* from its own tree without re-deriving it. A decision
  made in one room still has to arrive in the room that implements it. ⇒ **if you hold a packet, the
  hold is a row on the tracker, not a banner on the packet** — the tracker is what a later session
  reads. Same family as *a workaround in a runbook is a bug report nobody filed*, one artifact class
  over: the understanding was complete and the delivery was the missing step.
  ⚠ **And the thing that made it worse is free to check: a branch named for a counterpart reads as
  that counterpart's work.** `feature/entity-apps` here is **ours**, forked before the run-env arc
  and before the dev-history rewrite — 1,807 files of `dev` absent from it, **1** file added, **0**
  under `tools/run-env/`, merge base `e125910`. Merging it reverts work. **Its name is the only thing
  about it that points at the other seat**, and that was enough to make a status question unanswerable
  for a session. `git diff --name-status dev <branch> | awk '{print $1}' | sort | uniq -c` settles it
  in one command; run it before describing any branch as somebody else's.
- ⛔ **A LOCKFILE RESOLVED AGAINST A PATH DEP IS RESOLVED AGAINST WHATEVER IS ON DISK — INCLUDING
  ANOTHER SEAT'S HALF-FINISHED EDIT (2026-09-15, committed and reverted in one session).** `make lint`
  re-resolved `Cargo.lock` and it grew `entity-tree → entity-wire`; it was committed with a message
  saying it came *"from the live sibling moving under us: `entity-core-rust` is at `e388587`."* False
  in the only sense that matters — `git show e388587:core/tree/Cargo.toml | grep -c entity-wire` is
  **0** and the same grep on their working copy is **1**, with **12** uncommitted paths there. **The
  tell arrived on its own:** a *second* crate grew the same dependency minutes later with the kernel
  HEAD unmoved. ⇒ ***"the sibling moved" and "the sibling is mid-edit" are different facts, and only
  one of them is yours to record*** — a lockfile taken from the second is reproducible from no commit
  in either repo. **This is the build-pair entry's own subject arriving from a direction it does not
  name:** that entry is about the kernel being at an *unrecorded commit*; the worse case is its being
  at *no commit at all*, and `build-stamp`'s `-dirty` marker is on **our** half, so nothing flagged it.
  **Cheap standing check: `git -C ../entity-core-rust status --short` before committing any lockfile
  change you are about to attribute to a path dep.** If it is dirty the change is not yours; if you
  need the resolution anyway, take it under **`CORE_RUST_REF`**, which builds from a `git archive` of
  the committed kernel and therefore cannot see their edits — that is what settled it here (restored
  bytes + `make wasm CORE_RUST_REF=e388587` → stamp `e388587` with no `-dirty` on the kernel half, and
  `git diff <base> -- Cargo.lock` empty afterwards). **Construction outranks argument.**

**New packets:** `docs/status/ROUTING-<date>-<letter>-<recipient>-<slug>.md`, opening with `**To:**`
/ `**From:**` / `**cc:**` **each on its own line**, `To:` naming **repositories** (never a person or
a nickname), `cc:` meaning *you are not on the hook*. Never reuse a letter within a day.
**Cite a packet by its FULL stem** — `<date>-<letter>` is unique to one repo on one day, which is
not unique, and three ids in this ecosystem already reach three different packets each. Existing
documents stay where they are; this applies going forward.

⭐ **And an ASK ID has the identical defect — measured 2026-09-15.** `A-<n>` is unique to one *seat*
and therefore not unique: our `A-36` is FEED §2.4's cursor contradiction, `entity-workbench-go`'s
`A-36` is `FEED-R2`'s signature outside the publishable prefix, and on one day arch's register said
**"`A-36` HELD"** about ours while a packet said **"`A-36` is ruled"** about theirs. Both correct.
⇒ **qualify an ask id with its seat the moment a document is read by more than one** —
`browser-rust A-36`, `workbench-go A-36` — which is the spelling arch already uses in
`COHORT-OPEN-ITEMS` and did not use in the packet addressed to two seats. **The collision is
invisible from inside one tree**, which is why it has to be a habit rather than something you
notice: on your own board your own id is unambiguous, and the ambiguity is created by the reader.
**Never renumber to resolve one** — annotate, because a renumber makes one fact reachable by two
ids, which is the worse failure.

> **Where this convention is *not* written down, measured 2026-09-09.** Arch's instruction says it
> is already in `AGENTS-STANDARD.md`. It is in **arch's copy** — the canonical
> `<coordination-tree>/AGENTS-STANDARD.md` is 231 lines with **no** `## Routing packets` section,
> **ours is byte-identical to canonical**, and 14 of 16 repos here carry no such section. The only
> other holder is `entity-system-generator`, whose copy says so in its own provenance note (*"arch's
> local edit of 2026-09-08 … not yet reconciled"*). The standard forbids editing our copy, so **this
> section is where it lives for us** until the canonical file moves. Do not "fix" `AGENTS-STANDARD.md`
> here; propose it upstream.

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

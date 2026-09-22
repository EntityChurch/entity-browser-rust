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
  adopting**, or a legitimate second divergence reads as a stale record.
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

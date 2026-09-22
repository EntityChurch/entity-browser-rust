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
- **Host needs only `make` + `podman`** — every target MUST run in the image, the desktop
  GUI included. The one thing the host supplies that a container cannot is **a display**
  (Wayland or X11) for `make tauri-run`; `make host-run` is the run-native alternative and
  wants one GUI runtime lib. Why each target is containerized the way it is, which three
  podman flags are load-bearing and what breaks without them, and where a file-transfer
  share actually lands on disk: **`docs/agents/memory/containers-and-the-host.md`**.

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

## The earned-lesson ledger — `docs/agents/memory/`

**131 earned lessons live in `docs/agents/memory/`, opened by trigger and never read end to
end.** 128 of them were in this file, under a heading that said *Build & test* and described
none of them — 4,226 lines, 92% of the charter. That is **AP56**, *a fact filed under the
exception is not filed under the rule*, which this file used to teach about itself from inside
the section it described.

**Enter through [`docs/agents/memory/INDEX.md`](docs/agents/memory/INDEX.md).** Every entry was
earned by a shipped bug, a measurement, or a gate that was falsified; the move was verbatim and
the dates are unchanged.

| open this | when you are about to… |
|---|---|
| [`method-how-we-measure`](docs/agents/memory/method-how-we-measure.md) | write a gate, falsify one, run a scripted edit, or make a claim about code |
| [`boot-and-availability`](docs/agents/memory/boot-and-availability.md) | touch the boot path, `sw.js`, the build-update prompt, or a rollback slot |
| [`recovery-and-diagnostics`](docs/agents/memory/recovery-and-diagnostics.md) | touch System Recovery, the kill switch, the Problems card, or a failure detector |
| [`caching-and-foreign-bytes`](docs/agents/memory/caching-and-foreign-bytes.md) | store, refresh or re-encode bytes somebody else authored |
| [`deployment-ownership-and-succession`](docs/agents/memory/deployment-ownership-and-succession.md) | touch `/entity-deployment.json`, a home site, an origin registry, or a re-key |
| [`windows-state-and-hydration`](docs/agents/memory/windows-state-and-hydration.md) | add a window, persist window state, or read the tree at construction |
| [`peers-arms-and-storage`](docs/agents/memory/peers-arms-and-storage.md) | write across the Direct/Worker split, or encode an entity's `data` |
| [`connectivity-webrtc-and-liveness`](docs/agents/memory/connectivity-webrtc-and-liveness.md) | touch signaling, WebRTC establishment, keepalive or peer liveness |
| [`desktop-tauri`](docs/agents/memory/desktop-tauri.md) | change the desktop shell, its app server, or a service default |
| [`containers-and-the-host`](docs/agents/memory/containers-and-the-host.md) | change a `make` target's containerization, a serve path, or a host requirement |
| [`publishing-and-the-axes`](docs/agents/memory/publishing-and-the-axes.md) | change `publish`, an axis, the clean, `--verify`, or a signed root |
| [`content-sites`](docs/agents/memory/content-sites.md) | change the site vocabulary, an asset payload, or a stored wire shape |
| [`cross-impl-and-vocabulary`](docs/agents/memory/cross-impl-and-vocabulary.md) | compare bytes with another implementation, or mint a type tag |
| [`feed-and-the-social-tier`](docs/agents/memory/feed-and-the-social-tier.md) | touch the feed — reading, composing, mirroring, or the browse list |
| [`cross-repo-routing-and-claims`](docs/agents/memory/cross-repo-routing-and-claims.md) | state the status of another repo, or route a measurement to one |
| [`build-and-release`](docs/agents/memory/build-and-release.md) | touch the build pair, cache headers, the fleet probe, or a release artifact |
| [`testing-and-the-gates`](docs/agents/memory/testing-and-the-gates.md) | add a gate, run the e2e suite, or explain a red |

⚠ **A new lesson goes to the area file that matches it, never back to this one.** If none
fits, say so in the handoff — and if a second session says the same, that is the evidence for a
new area file, not for an appendix here.

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

**New packets go to `docs/outbox/`** — packets we have SENT, and nothing else; archived to
`docs/archive/outbox/` once acknowledged. The 132 `ROUTING-*` files still under `docs/status/`
predate that split and stay where they are.
`docs/outbox/ROUTING-<date>-<letter>-<recipient>-<slug>.md`, opening with `**To:**`
/ `**From:**` / `**cc:**` **each on its own line**, `To:` naming **repositories** (never a person or
a nickname), `cc:` meaning *you are not on the hook*. Never reuse a letter within a day.
**Cite a packet by its FULL stem** — `<date>-<letter>` is unique to one repo on one day, which is
not unique, and three ids in this ecosystem already reach three different packets each. Existing
documents stay where they are; this applies going forward.

**What a held packet costs, why an ask id is not unique across seats, and the lockfile a path
dep resolved against another seat's uncommitted edit:**
`docs/agents/memory/cross-repo-routing-and-claims.md`.

> **Where this convention is *not* written down, measured 2026-09-09.** Arch's instruction says it
> is already in `AGENTS-STANDARD.md`. It is in **arch's copy** — the canonical
> master copy of `AGENTS-STANDARD.md` is 231 lines with **no** `## Routing packets` section,
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

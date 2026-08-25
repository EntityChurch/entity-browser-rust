# entity-browser-rust — status

_Updated: 2026-08-23 · public: **v0.8.0** (`master`) · working branch `dev`, version **0.8.2**_

This is the tracker: where the product is, what is proven and on what, what blocks the next
release, and what is open. It is a **published** document — so it cites files, symbols and
measurements rather than commit SHAs, which do not resolve for a reader outside this tree
(see *Commit pins* below).

**Deeper indexes, so this file does not restate them:**

| For | Read |
|---|---|
| What blocks the release, ordered | `docs/plans/PUNCHLIST-RELEASE.md` |
| Release mechanics, matrix, open edges | `docs/RELEASE-READINESS.md` |
| Deferred work with the reason | `docs/plans/BACKLOG.md` |
| Connectivity open items (canonical table) | `docs/architecture/reviews/BUILDOUT-SIGNALING-AND-NETWORK-EXTENSIONS.md` §8 |
| What changed for a user | `CHANGELOG.md` |
| Where the last session left off | `docs/status/HANDOFF-2026-08-23-c-full-screen-wake-lock-and-a-concession-spent.md` |

Spot-check a row before acting on it. That table has carried stale rows, and they survived by
being read instead of run.

## Where it is

`entity-browser-rust` is the DOM-primary Rust/WASM **reference application** on the entity-core
substrate: a window-manager / UI shell over the entity tree, rendered as HTML DOM in the browser
(WASM peer) and in a Tauri desktop WebView. It is a *binding / app* built on the Rust reference
implementation and its SDK — one worked example of the paradigm, not a mandate.

Maturity is **research preview**: suitable for evaluation and exploration, not a hardened
production deployment. HTML DOM is the only render path; the legacy native/egui renderer is gone
and `make native` prints a deprecation redirect.

## Gate state

Re-measured 2026-08-23 rather than quoted:

| Gate | Result |
|---|---|
| `make test` | **1283 / 0 / 8-ignored** across 15 test binaries |
| `make test-tauri` | **55 / 0** across 4 binaries — re-run today; `src-tauri` is workspace-excluded, so it is not in the number above |
| `make lint` | clean — clippy, ui-lint, i18n-lint, i18n-locale-check, i18n-callsite-check, i18n-untranslated, tree-hygiene |
| `make e2e-worker` | **25 passed / 0 failed, 374.28 s** — unfiltered, end of session (+1 test: the R1 milestone-trace gate). An earlier unfiltered run the same afternoon was 23/1, the failure being blocker 5; it did not reproduce across 13 subsequent runs |

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
strings — and conflating them is what made the first write-up unreadable. Record:
`docs/status/STATUS-2026-08-23-b-the-i18n-backlog-is-translated-and-the-ratchet-is-an-allowlist.md`.

**Say which suite you mean, and re-run before quoting.** These numbers have gone stale in hours,
repeatedly. The 8 ignores are 4 `crossimpl_go_live` (needs core-go's live publisher), 3 fixture
emitters the e2e drives with `--ignored`, and 1 live-backend upload. Note that `make test`
compiles `tests/e2e_worker.rs` to **nothing** (`#![cfg(feature = "e2e")]`), so a green `make test`
is no evidence that file even parses.

## What is proven, and on what

Every connectivity gate is podman containers on a single Linux box. Each rig states its own scope;
none of them is two physical machines on real networks.

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

**The headline gap is unchanged: nothing has been run on two computers.** The product's actual
pitch — download Tori, run the signaling node yourself, connect your machines to it — has never
been executed end to end by a person on real hardware over a real network. That is not a
regression; it is the class of thing that has to be found by running it.

## Release prep — what this repo did, and where the line is

**The GitHub Actions half is not this repo's to finish.** The workflow gets tweaked and proven on
a branch by whoever owns the release pipeline; the local build is unaffected either way and works
today (`make dist`, `make dist-web`, host needs only `make` + `podman`). What is owed *here* is
that the tree be correct and honest before it is promoted. Done 2026-08-23:

- **`CORE_RUST_REF` is `v0.8.2`** — a public tag, not a dev SHA. Published commits are authored
  fresh at the release boundary (ADR-0027), so an entity-core-rust `dev` SHA never resolves for
  anyone else and no push of theirs would have rescued the old pin. The tag is anticipated: it can
  sit in the file before it exists.
- **`release/0.8.2` carries a reduced `release.yml`** — `web` + `assemble` — because `.github/`
  does not exist on the public `master` at all, so a tag today would trigger nothing. `dev` keeps
  the full platform matrix as the one home of the platform list (ADR-0023).
  **`assemble` is kept, trimmed to `needs: [web]`, and that part is not optional:**
  `upload-artifact` produces a *workflow artifact* (90-day expiry, attached to nothing) and
  `gh release create` lives in `assemble` — so a `web`-only workflow with no assemble yields no
  GitHub Release and no downloadable file, which is the bare tag the reduction exists to avoid.
- **The notes footer and the CHANGELOG no longer advertise five installers this release does not
  carry**, and the footer no longer links a doc the publish pipeline scrubs.

## Release blockers

1. **`CORE_RUST_REF: v0.8.2` is anticipated — the tag is cut as part of the release, not before
   it.** Nothing to chase; noted here only so the *second* half of the pin check is not forgotten.
   The ref has to be publicly resolvable **and** carry the symbols we call — `NegotiationReport`,
   `IceObserver`, `with_ice_observer`. Both superseded pins failed one of those: `7528a9f` and
   `302b7f4` were dev SHAs (unresolvable), and `302b7f4` additionally predated `NegotiationReport`
   by five commits, so it would have failed at compile on five platforms rather than loudly at
   checkout. **A tag cut before those symbols land is exactly as dead as the SHA it replaced** —
   `git grep <symbol> $REF -- bindings/` is the check, and it is not retired by using a tag.
   This affects **CI only**; the local build resolves the sibling checkout on disk and is
   unaffected. Detail: `docs/RELEASE-READINESS.md`.
2. **Icons are a placeholder upscale.** The Windows build is unblocked (the set is generated and
   wired into `src-tauri/tauri.conf.json`), but the icon a user actually sees is not done. Replace
   `src-tauri/icons/icon.png` with real 1024×1024 art and re-run `cargo tauri icon`.
3. **The macOS legs and the Windows `.msi` are written but never executed** — and they are exactly
   the artifacts that cannot be produced from Linux. Prove them with a `workflow_dispatch` dry run
   before the first tag that depends on them. Windows NSIS **is** proven, cross-built here.
4. **linux-arm64 is unproven** — the binaryen pin is arch-resolved but has not run on an aarch64
   host.
5. **R1 — the display-gated Tauri WebView phase. Red 3/3 then green 13/13 on one unchanged tree,
   2026-08-23. Open, not reproducible, and the trigger is unidentified.** Full record:
   `docs/status/FINDING-2026-08-23-r1-is-a-stall-in-wasm-instantiate-not-a-slow-one.md`. The
   session ended on a full unfiltered **25 / 0 in 374.28 s** with the phase green at 1088 ms.
   - **Two candidate causes are DEAD, both killed by controls.** *Host load* — wrong: the figures
     quoted for it were a load average not divided by 32 threads and `free` rather than
     `available` (77 GB). *The container memory cap* — wrong: raising it looked decisive until the
     plain-defaults control went green, then six more at defaults. **A fix a control reproduces
     without the fix is not a fix.**
   - **It is not the i18n round** — bisected against a pre-translation build, which fails
     identically (+27 KB of catalogs, +78 KB of wasm, 0.27 %).
   - **What is established is WHERE it stops, not why.** On every red the child's stdout ends at
     the `instantiateStreaming` fallback and goes silent for 60 s, and **zero of the five boot
     milestones the module logs from its own entry point arrive** — while the console bridge is
     demonstrably forwarding other WebView output. So the module never begins executing: a stall
     in `WebAssembly.instantiate`, not slow app boot and not a panic. That now rests on a
     **positive control** — a green run reports `5/5` — rather than on reasoning from absence.
   - **What was fixed is the diagnostic, not the product.** The phase now prints the milestone
     trace on failure (`✓ step @ N ms` / `✗ step — never seen`, with an explicit *never started*
     case) **and the count on success**, so the next red is readable in one line instead of a
     session. Gate: `the_boot_milestone_trace_distinguishes_never_started_from_hung_midway`,
     mutation-checked both ways.
   - **The `application/wasm` question is live but not urgent.** Tauri's asset protocol serves
     `frontendDist` without the type, which forces the non-streaming path;
     `src-tauri/src/app_server.rs` already gets this right and has a test pinning it. The earlier
     ruling that a 48× margin refutes it was wrong **in kind** — a margin bounds *slowness* and
     says nothing about a path that never completes — but "plausible mechanism" is not "found".
   - **Scope:** this is the **debug** bundle at 29.5 MB; `make dist` ships `opt-level=z` + LTO +
     `wasm-opt -Oz` and has never been measured on this path. The desktop is not shown broken for
     users by a rig failure against a debug artifact — nor shown fine.

## Pre-ship QA — human device verification, not agent-doable

1. **Optimized bundle on Safari / iOS.** Engineering is done: the binaryen **119** pin in the
   `Dockerfile` fixes the reference-types funcref mis-optimization that threw `Table.grow`
   RangeError on JavaScriptCore, and `make wasm-release` builds clean through it. **Remaining:**
   deploy the rebuilt bundle and open it on a real iPhone and desktop Safari.
2. **IndexedDB across-restart durability on WebKitGTK — ✅ verified.** Confirmed by hand via
   `make tauri-run`: create a site, save, relaunch, the site is still there. Safari/iOS on real
   hardware still wants the same check; the WebKitGTK/JavaScriptCore question is answered.
3. **Full screen and wake lock in the Tauri WebView.** Both are Firefox-gated here. WebKitGTK is
   a different engine and has surprised this repo badly before (it ships no `RTCPeerConnection` at
   all). Both degrade honestly — the full-screen button renders only when
   `document.fullscreenEnabled` is true, and a wake-lock request on an engine without the API is a
   no-op the app already handles — so the outcomes to tell apart are *absent* and *works*.
   `make tauri-run`, open Apps, launch something, press ⛶ and leave it running.
4. **Two real devices on two real networks.** Open the SPA on two devices on two different
   networks, `meet tag <label>` on both, offer a file from one and pull it from the other. A
   failure here is data, not a regression, and it is the next thing that would change the roadmap.
   What no rig on this box can produce: a phone on cellular, a captive portal, an ISP CGNAT, or a
   symmetric NAT.

## Commit pins — why this file has none

ADR-0027 authors every published commit **fresh at the release boundary**, so public history is a
different history from `dev`. A `dev` SHA has therefore never resolved for an outside reader and
never will — and repointing one at another `dev` SHA changes nothing for them.

This file used to carry **15** such pins, the largest single concentration in anything this repo
publishes. They are gone, replaced by the file, symbol, or measurement the citation was actually
resting on. Measured exposure across the repo is **48**, of which only 18 are countable from
`CANONICAL-DOCS.toml` — source comments and prose outside a `docs/` root are *always kept* by the
publish filter and appear in no manifest. Detail and the numbers: `docs/RELEASE-READINESS.md`.

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

**Content / publishing**
- **Cross-domain `site:` links — the fail-loudly half is CLOSED on both sides.** Measured
  2026-08-23: the corpus carried **seven**, three files, all on `entity-church-foundation`, all
  outbound; under per-domain publishing they resolve against the publishing peer and 404.
  `entity-core-papers` swept them to absolute URLs (`ef3f662`) and added its own gate; our
  exporter **reports** every out-of-set target and now **refuses by default**. Re-verified here
  rather than taken on report: **0** cross-domain refs across all five domains, all four domains
  publishing clean under the refusal (412 pages, 0 dangling), an injected target refused with 7
  errors and exit 1. Hatch: `ALLOW_OUT_OF_SET_LINKS=1`; `STRICT_LINKS=1` is accepted and is now a
  no-op. Making such a link *work* is backlog **B-4** —
  `docs/plans/DESIGN-CROSS-DOMAIN-SITE-LINKS.md`, which carries the questions for arch.
- **Latent, filed not fixed:** the Registry Browser writes a resolved origin under
  `system_peer_id()` while the Site Browser reads under its **bound** peer. They coincide only
  because `system_peer_id()` is still an alias for `primary_peer_id`; a Site Browser on any
  non-primary peer gets `Unreachable` on a name that resolved. Same shape as the registry-pin bug.

**Build & release**
- `dist/` hygiene — ship `make wasm-release` with default features, never a debug or `demo-apps`
  `dist/`.
- Vault label `|` / newline is not escaped (`vault_codec`); do it in a calm window alongside input
  validation.
- ~~Commit `Cargo.lock` for reproducible release builds~~ — **closed**, both lockfiles are tracked.

**Long-deferred capability stages** (`docs/architecture/specs/SYSTEM-VISION.md`): KB wiki PoC, type renderer registry,
pipeline builder (SDK Layer 2), relay on the Tauri backend, the capability + identity arc (Key
Manager stays a placeholder until then), cross-renderer portability, self-modification. Pull into
the roadmap when scoped.

## Next

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

## Waiting on

- **`entity-core-rust`** — this crate uses path dependencies into `../entity-core-rust/`, so the
  build fails at dependency resolution if that checkout is missing or at an incompatible revision.
  SDK-tier changes belong upstream there, not here. The public push of the pinned revision is
  release blocker 1 above.

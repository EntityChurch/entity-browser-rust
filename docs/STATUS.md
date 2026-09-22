# entity-browser-rust — status

_Updated: 2026-09-03 · version **0.9.0** in the manifests; the next tag's number is not yet
decided — see `docs/status/CHECKPOINT-2026-09-03-release-closeout.md`_

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

## Gate state

Re-measured **2026-09-03 (release closeout)** rather than quoted — `test`, `test-tauri` and `lint`
all re-run from a clean tree at `dev` HEAD in the same pass:

| Gate | Result |
|---|---|
| `make test` | **1484 / 0 / 17-ignored** across **17** test binaries — re-measured 2026-09-03 at this tip. It has moved 1300 → 1484 over ten days. Re-run it; do not quote this line |
| `make test-tauri` | **56 / 0** across 4 binaries — **re-measured 2026-09-03**, not quoted forward. (`src-tauri` is workspace-excluded, so it is not in the number above.) |
| `make lint` | **exit 0, re-measured 2026-09-03 — eleven checks**, not the seven this row carried through 0.9.0. The two added since: `cargo clippy --features e2e --tests`, which is the only thing in the tree that compiles `tests/e2e_worker.rs` at all, plus the cache-policy pair. `ui-lint` atoms=7 styles=135 hex=4 across 23 files · `net-lint` matches baseline · `foreign-cache-lint` matches baseline · `cache-policy-lint` 36 shared vectors (9 immutable / 27 mutable) · `cache-policy-doc-check` ok · `i18n-lint` **raw=0** phys=0 · `i18n-locale-check` 30 locales × **769** keys · `i18n-callsite-check` **699** call sites · `i18n-untranslated` 6 allowlisted · `tree-hygiene` no tracked path is gitignored |
| `make e2e-worker` | **64 passed / 0 failed, 674.73 s** — unfiltered, run **2026-09-03 at `3b96b47`**, the `sw.js` rollback fix, *with* that change in. **Not re-run at this tip, and that is deliberate rather than an omission:** every commit since is documentation, so no input to the suite has moved. Said explicitly so three fresh numbers above do not imply a fresh fourth |

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
strings — and conflating them is what made the first write-up unreadable. Record:
`docs/status/STATUS-2026-08-23-b-the-i18n-backlog-is-translated-and-the-ratchet-is-an-allowlist.md`.

**Say which suite you mean, and re-run before quoting.** These numbers have gone stale in hours,
repeatedly. The 17 ignores are 4 `crossimpl_go_live` (needs core-go's live publisher), **11 fixture
emitters** the e2e drives with `--ignored`, 1 static-export demo emitter, and 1 live-backend upload —
enumerated 2026-09-03, because "8 ignored" sat here while the fixture set nearly doubled behind it.
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
`docs/RUNBOOK-TWO-MACHINES.md` §5 is a transcript now, not a plan, and that run is what exposed six
defects no gate we own could see (record:
`docs/status/STATUS-2026-08-21-two-browsers-chatted-and-moved-a-file-through-a-desktop.md`).

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
   moving a file, was run on real hardware on 2026-08-21 and is a transcript in
   `docs/RUNBOOK-TWO-MACHINES.md` §5. What is untested is the cross-*network* case — one ISP to
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
     desktop WebView ships **no `RTCPeerConnection` at all**
     (`docs/status/FINDING-2026-08-22-the-linux-desktop-webview-has-no-webrtc.md`), so offer/pull is
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
   - **Arch has already scoped the fix and we are not proposing a mechanism.** Their
     `docs/research/explorations/EXPLORATION-NON-INTERACTIVE-FRESHNESS-AND-ANTI-REPLAY.md` §B4 names
     this seam (*"canonical and cross-referenced but un-quantified"*) and **Knob 2** is exactly this
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

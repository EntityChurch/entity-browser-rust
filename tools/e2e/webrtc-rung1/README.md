# rung-1 WebRTC — two real browser peers over a signaling node

Proof-of-concept harness that drives **two real entity browser peers** through the §6.5 WebRTC
establishment path and reports where it gets to. Built 2026-08-04 while validating the WebRTC leg;
it is the reproduction behind
[`docs/status/ROUTING-2026-08-04-webrtc-rung1-rendezvous-encoding-mismatch-to-core-rust.md`](../../../docs/status/ROUTING-2026-08-04-webrtc-rung1-rendezvous-encoding-mismatch-to-core-rust.md).

**Current state (2026-08-05): BIDIRECTIONAL green — this is now a fail-closed gate.** Both browsers
boot Worker mode, confirm the establisher, auto-escalate on a cross-peer `exec`, rendezvous at the
§3.2 `pair` key, open an `RTCDataChannel`, and complete a cross-peer round-trip in **both** directions
(`status=200`) over the one §6.5 channel — including the §6.5 (b) reciprocal grant (mutual minting)
that lets the *answerer* originate back. Re-verified against core-rust HEAD `f227df8` under the
§7-narrowed grant (reciprocal mint gated on `established_via_rendezvous_key`). The two earlier walls
are both resolved: the id-encoding never-meet (`pair_key`/`glare_role` self=base58 vs
target=`ecfv1-sha256:`) and the ICE/DTLS channel-close. See the `STATUS-2026-08-05-webrtc-rung1-*`
docs for the arc.

**Run it as a gate:** `make e2e-webrtc` (host podman) — stands up the topology, drives both browsers,
tears everything down, and **the BIDIRECTIONAL verdict is the exit code** (both channels open AND both
directions `status=200`; a lone direction is a FAIL, not a known gap).

The drive is still a Python/WebDriver spike (no Rust compile for the drive), kept because it encodes
hard-won substrate facts and because the two-container-on-a-bridge topology has no equivalent in the
single-session Selenium harness `e2e-worker` uses. A future `tests/e2e_worker.rs` port (two fantoccini
clients) would fold the assertions into the Rust suite; until then `make e2e-webrtc` is the gate.

## Substrate facts (load-bearing — see AGENTS.md "WebRTC / two-browser" gotcha)

- **Browser↔browser only.** The native peer has no `RTCDataChannel` (it UDP hole-punches); a real
  data channel is browser-to-browser.
- **Shared user-defined podman bridge, NOT host-net + default.** Rootless pasta mirrors the host IP
  into a default-network container, so a host-net + bridge pair advertises colliding candidates and
  ICE fails. Put **both** firefox containers on one `podman network create` bridge → distinct
  routable IPs.
- **Firefox prefs:** `media.peerconnection.ice.obfuscate_host_addresses=false` (raw-IP host
  candidates; mDNS `.local` won't resolve cross-container) and, because a bridge origin is reached
  via `host.containers.internal` (**not** a secure context), `dom.securecontext.allowlist` +
  `dom.securecontext.whitelist = host.containers.internal` — without it OPFS is absent, the Worker
  bootstrap fails, and the app silently falls back to Direct mode (disabling the worker-only §6.5
  establisher).
- Bridge containers reach host services (dist server, signaling node) via `host.containers.internal`.

## Run it

Prereq: `make wasm` has produced `dist/`; the pinned selenium image is pulled (see `../README.md`).

```bash
make e2e-webrtc              # THE GATE: teardown → setup → drive → teardown; verdict = exit code
make e2e-webrtc BUILD=1      # rebuild dist/ first
```

Or drive the rig directly (leaves containers up for inspection — the manual-debug path):

```bash
bash tools/e2e/webrtc-rung1/rung1_repro.sh            # set up everything + drive (exits non-zero on FAIL)
bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown   # remove containers + network + host procs
```

It runs a **build-skew preflight** (asserts `dist/`, rebuilds `entity-signaling-node` from the current
`entity-core-rust` HEAD, prints that SHA), then creates the bridge network, two firefox containers
(`:4446`/`:4447`), a host dist server on `:8092`, launches the node, runs the drive, and prints the
per-`collect` `included_count` from the node log. `SKIP_NODE_BUILD=1` reuses an existing node binary.

### Build skew is now load-bearing (§6.5 is Require)

core-rust raised §6.5 to **Require** (`007e078`): the browser leg **refuses** SDP that has not passed
§6.3 signature verification. So a `VerificationUnavailable` on our leg means **mixed builds, not a NAT
problem**. The two browser peers can't skew — both load one host-served `dist/`, identical by
construction — but the **signaling node is a separately-built binary**: a stale
`target/debug/entity-signaling-node` against a fresh `dist/` (or vice versa) is exactly that §6.3/§6.5
mismatch, and it reads as "ICE failed." The preflight pins both to the same core-rust HEAD; keep them
there. To debug rung-1 under the tolerant posture, revert Require upstream (one line) and rebuild both.

### Seeing worker-side WebRTC logs

The §6.5 establisher runs in the **worker**, whose console is a separate realm the main-thread capture
(`window.__entity_browser_log`) never saw — so a worker-only failure read as a silent stall. Worker
lines (incl. the seam-guard `warn!` and any `VerificationUnavailable`) are now **forwarded** to that
buffer over a same-origin `BroadcastChannel` (`assets/entity-worker-loader.js` posts, `index.html`
drains), tagged `source:"worker"`. `spike_rung1_integration.py`'s `grep_log` and `diag_webrtc_log.py`
pick them up unchanged — the eventual loud-red is a readable line, not a worker-side black box.

## Files

| File | What |
|---|---|
| `rung1_repro.sh` | One-command orchestrator (setup → drive → report; `teardown`). |
| `spike_rung1_integration.py` | The WebDriver drive: boot both browsers with webrtc URL params, read each primary id, open a Shell in each, symmetric concurrent `exec entity://<other>/system/tree list`, poll scrollback. |
| `diag_webrtc_log.py` | Boots one browser and dumps every webrtc-related main-thread log line (how the secure-context / Direct-fallback blocker was found). |
| `spike_e2a_loopback.py` | Falsifier: in-page two-`RTCPeerConnection` loopback — proves headless Firefox WebRTC works at all. |
| `spike_e2b_crosscontainer.py` | Falsifier: bare cross-container host-candidate WebRTC (no entity stack) — proves the network path. Run against a shared-bridge pair. |
| `rendezvous_check/` | Standalone `entity-signaling` **consumer** (no upstream modification) that computes `glare_role`/`pair_key` for the live peer ids across all four encodings — the root-cause proof table in the routing doc. |

The webrtc URL params the app reads (`src/session_config.rs`): `?worker=1` (webrtc is worker-only) +
`&webrtc_node=ws://<host>:<port>&webrtc_node_peer=<node-id>&webrtc_enable=1`.

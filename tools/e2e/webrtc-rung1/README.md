# rung-1 WebRTC — two real browser peers over a signaling node

Proof-of-concept harness that drives **two real entity browser peers** through the §6.5 WebRTC
establishment path and reports where it gets to. Built 2026-08-04 while validating the WebRTC leg;
it is the reproduction behind
[`docs/status/ROUTING-2026-08-04-webrtc-rung1-rendezvous-encoding-mismatch-to-core-rust.md`](../../../docs/status/ROUTING-2026-08-04-webrtc-rung1-rendezvous-encoding-mismatch-to-core-rust.md).

**Current state:** the pipeline stands up end-to-end — both browsers boot Worker mode, confirm the
establisher, auto-escalate on a cross-peer `exec`, connect+authenticate to the signaling node, and
exchange `offer`/`collect` — but the **pair rendezvous never matches** (`collect` always returns
`included_count=0`), so no `RTCDataChannel` forms. Root cause: an id-encoding asymmetry into
`entity-signaling`'s `pair_key`/`glare_role` (self = base58, dial target = `ecfv1-sha256:` author
hash). See the routing doc. This is upstream; the harness is the reproduction, not a passing test.

These are throwaway Python/WebDriver spikes (no Rust compile for the drive), kept because they encode
hard-won substrate facts. The eventual real coverage is a `tests/e2e_worker.rs` test — this is the
scouting that de-risked it.

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
bash tools/e2e/webrtc-rung1/rung1_repro.sh            # set up everything + drive
bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown   # remove containers + network + host procs
```

It creates the bridge network, two firefox containers (`:4446`/`:4447`), a host-run
`entity-signaling-node`, a host dist server on `:8092`, then runs the drive and prints the per-`collect`
`included_count` from the node log.

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

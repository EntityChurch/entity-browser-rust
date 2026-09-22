#!/usr/bin/env bash
# Self-contained rung-1 WebRTC reproduction: two real entity browser peers on a
# shared podman bridge, a host-run signaling node, host-served dist. Drives the
# symmetric cross-peer exec and reports where the flow gets to.
#
# Prereqs: `make wasm` has produced dist/. Run from the entity-browser-rust root.
# Usage:  bash rung1_repro.sh            # set up + run
#         bash rung1_repro.sh teardown   # remove containers + network + procs
set -euo pipefail

IMG=docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404
IMG_CHROME=docker.io/selenium/standalone-chrome:latest

# ENGINE_A / ENGINE_B — which browser engine each side runs. Default is the
# historical firefox/firefox, so every existing gate is unchanged.
#
# This exists because Firefox↔Firefox is the ONE pair that cannot exhibit a
# data-channel message-size defect: it negotiates a ~1 GiB `maxMessageSize` and
# fragments internally, where Chromium advertises 262 144 and does not. A
# transfer that stalled on a real Android(Chrome) → desktop(Firefox) pair was
# invisible to every gate here, not because the gates were weak but because the
# failing configuration was not in the population. The spike reads the same two
# variables to build capabilities and PRINTS what the grid actually started, so
# a mixed run that quietly came up same-engine cannot report a cross-engine pass.
ENGINE_A="${ENGINE_A:-firefox}"
ENGINE_B="${ENGINE_B:-firefox}"
export ENGINE_A ENGINE_B
img_for() { case "$1" in chrome) echo "$IMG_CHROME";; firefox) echo "$IMG";; *) echo "!! unknown engine '$1'" >&2; exit 1;; esac; }
IMG_A="$(img_for "$ENGINE_A")"
IMG_B="$(img_for "$ENGINE_B")"

NET=entity-rtc-spike
# TOPOLOGY=shared (default) — both browsers on ONE bridge, so their host
#   candidates are mutually routable. This is the positive rig: it proves the
#   §6.5 mechanism and the shipped meet-then-chat path.
# TOPOLOGY=split — one ISOLATED network per browser, no route between them, and
#   the signaling node reachable only through the host. This is the negative
#   control: it is what "two peers behind different NATs" looks like from the
#   app's point of view, and it is the instrument that MEASURES the ICE gap
#   instead of reasoning about it. `resolve_webrtc_provisioning` hardcodes
#   `ice_servers: Vec::new()`, so there are host candidates and nothing else.
#
# The shared rig going green is NOT evidence of internet reachability — the two
# containers are on one subnet, where host candidates always work. That is the
# claim this split mode exists to bound.
TOPOLOGY="${TOPOLOGY:-shared}"
# The spike reads it too — `survives idle` is only meaningful with a NAT in path
# and refuses to claim anything without one, so it has to know which rig it is in.
export TOPOLOGY
NET_A=entity-rtc-nat-a
NET_B=entity-rtc-nat-b
CORE=../entity-core-rust
# Overridable. On a host with no `cargo` the build step below uses
# `make e2e-signaling-node` (inside the image) and points this at its output.
SIG="${SIG:-$CORE/target/debug/entity-signaling-node}"
DISTPORT=8092
WSPORT=4071
SCRATCH="$(dirname "$0")"

teardown() {
  echo ">> teardown"
  podman rm -f rtc-a rtc-b >/dev/null 2>&1 || true
  podman network rm "$NET" >/dev/null 2>&1 || true
  podman network rm "$NET_A" "$NET_B" >/dev/null 2>&1 || true
  bash "$(dirname "$0")/nat_topology.sh" down >/dev/null 2>&1 || true
  pkill -f "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT" 2>/dev/null || true
  pkill -f "http.server $DISTPORT" 2>/dev/null || true
  echo ">> done"
}
[ "${1:-}" = "teardown" ] && { teardown; exit 0; }

# --- Build-skew preflight (load-bearing since §6.5 raised to Require) ----------
# core-rust flipped §6.5 to Require (007e078): the browser leg now REFUSES SDP
# that has not passed §6.3 signature verification. A `VerificationUnavailable`
# on our leg then means MIXED BUILDS, not a NAT/transport problem. The two
# browser peers can't skew (both load one host-served dist/ — same bytes by
# construction), but the SIGNALING NODE is a separately-built binary: a stale
# `target/debug/entity-signaling-node` against a fresh dist/ (or vice versa) is
# exactly the §6.3/§6.5 mismatch that reads as "ICE failed." So: assert dist/,
# pin both to the SAME core-rust HEAD, rebuild the node, and print the SHA so a
# §6.x refusal is attributable rather than mistaken for the network path.
# (worker bundle is unhashed; the app bundle is content-hashed, so assert on the
# stable worker wasm + index.html — the rig is worker-mode, so both are needed.)
{ [ -f dist/entity-worker_bg.wasm ] && [ -f dist/index.html ]; } || \
  { echo "!! dist/ not built — run 'make wasm' first (both peers load it)"; exit 1; }
CORE_SHA=$(git -C "$CORE" rev-parse --short HEAD 2>/dev/null || echo "unknown")
CORE_DIRTY=$(git -C "$CORE" status --porcelain 2>/dev/null | head -1)
if [ "${SKIP_NODE_BUILD:-}" = "1" ]; then
  echo ">> SKIP_NODE_BUILD=1 — reusing existing node binary (ensure it matches dist/'s core-rust build)"
  [ -x "$SIG" ] || { echo "!! $SIG missing and build skipped"; exit 1; }
else
  if command -v cargo >/dev/null 2>&1; then
    echo ">> rebuilding signaling node from core-rust HEAD ($CORE_SHA) to match dist/"
    ( cd "$CORE" && cargo build -p entity-signaling-node 2>&1 | tail -2 )
  else
    # A make+podman host has no cargo. Build the same crate inside the image,
    # into this repo's target dir, from the same sibling checkout dist/ links.
    echo ">> no host cargo — building the signaling node in the image ($CORE_SHA)"
    make --no-print-directory e2e-signaling-node || exit 1
    SIG="$PWD/target/e2e-node/debug/entity-signaling-node"
  fi
fi
echo "── build posture ─────────────────────────────────────────────"
echo "   core-rust HEAD : $CORE_SHA${CORE_DIRTY:+  (working tree DIRTY — node/dist may disagree)}"
echo "   §6.5 is Require: a VerificationUnavailable on the browser leg = MIXED"
echo "   BUILDS, not NAT. dist/ (make wasm) and the node above MUST come from"
echo "   this same core-rust checkout. To debug rung-1 under the tolerant"
echo "   posture, revert Require upstream (one line) and rebuild both."
echo "──────────────────────────────────────────────────────────────"

if [ "$TOPOLOGY" = "nat" ]; then
  # The positive traversal rig: two peers behind two SEPARATE NATs, each with
  # its own external address, plus a self-hosted STUN responder. See
  # nat_topology.sh for what kind of NAT this is and why that bounds the claim.
  echo ">> TOPOLOGY=nat — two peers, two routers, two external addresses"
  # UDP_TIMEOUT rides through to the routers' conntrack. The idle gate lowers it
  # so its quiet window can outlast a mapping; everything else takes the default.
  UDP_TIMEOUT="${UDP_TIMEOUT:-180}" bash "$SCRATCH/nat_topology.sh" up
  export UDP_TIMEOUT="${UDP_TIMEOUT:-180}"
elif [ "$TOPOLOGY" = "split" ]; then
  echo ">> TOPOLOGY=split — one ISOLATED network per browser (the NAT negative control)"
  podman network exists "$NET_A" || podman network create --opt isolate=true "$NET_A" >/dev/null
  podman network exists "$NET_B" || podman network create --opt isolate=true "$NET_B" >/dev/null
  podman container exists rtc-a || podman run -d --rm --name rtc-a --network "$NET_A" -p 4446:4444 "$IMG_A" >/dev/null
  podman container exists rtc-b || podman run -d --rm --name rtc-b --network "$NET_B" -p 4447:4444 "$IMG_B" >/dev/null
else
  echo ">> shared bridge network"
  podman network exists "$NET" || podman network create "$NET" >/dev/null
  echo ">> two browser containers on the bridge (distinct routable IPs): A=$ENGINE_A B=$ENGINE_B"
  # `--shm-size=2g`: Chromium's renderer dies on the 64 MiB default /dev/shm and
  # presents as a session that starts and then goes away mid-run. Harmless to
  # Firefox, so it is unconditional rather than a branch that only the mixed rig
  # exercises.
  podman container exists rtc-a || podman run -d --rm --name rtc-a --shm-size=2g --network "$NET" -p 4446:4444 "$IMG_A" >/dev/null
  podman container exists rtc-b || podman run -d --rm --name rtc-b --shm-size=2g --network "$NET" -p 4447:4444 "$IMG_B" >/dev/null
fi
for p in 4446 4447; do
  for i in $(seq 1 30); do curl -s -m2 localhost:$p/status 2>/dev/null | grep -q '"ready": *true' && break; sleep 1; done
done
A_IP=$(podman inspect rtc-a --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
B_IP=$(podman inspect rtc-b --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
echo "   rtc-a=$A_IP   rtc-b=$B_IP"

# --- The control has to be controlled ----------------------------------------
# Assert the topology we THINK we built. A split rig whose isolation silently
# failed is worse than no rig: it would "prove" host candidates traverse NATs.
# So probe the actual path A->B and fail loudly if it disagrees with TOPOLOGY.
# (Shared is probed too — if the bridge ever stopped being mutually routable,
# every green run of the positive gate would have been measuring nothing.)
probe_a_to_b() {
  podman exec rtc-a timeout 8 curl -s -m 5 -o /dev/null -w '%{http_code}' \
    "http://$B_IP:4444/status" 2>/dev/null || true
}
echo ">> probing the A->B path (the rig's own control)"
PROBE=$(probe_a_to_b)
if [ "$TOPOLOGY" = "nat" ]; then
  : # already probed above; probe_a_to_b is informational here
elif [ "$TOPOLOGY" = "split" ]; then
  if [ "$PROBE" = "200" ]; then
    echo "!! ISOLATION LEAKED: rtc-a reached rtc-b at $B_IP (HTTP 200)."
    echo "   The split rig would measure nothing — a media path exists that a"
    echo "   NAT'd pair would not have. Check 'podman network inspect $NET_A'"
    echo "   for isolate=true, and that no other network joins both containers."
    exit 1
  fi
  echo "   A->B blocked (curl said '${PROBE:-timeout}') — no direct path, as required"
else
  if [ "$PROBE" != "200" ]; then
    echo "!! rtc-a could NOT reach rtc-b at $B_IP on the shared bridge (got '${PROBE:-timeout}')."
    echo "   The positive rig assumes mutually routable host candidates; without"
    echo "   that this run proves nothing about the §6.5 mechanism."
    exit 1
  fi
  echo "   A->B reachable (200) — host candidates are mutually routable, as required"
fi

echo ">> signaling node (debug) on :$WSPORT"
pkill -f "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT" 2>/dev/null || true
sleep 0.5
# entity_signaling=debug is load-bearing: the per-bucket `signaling offer:` /
# `signaling collect` lines (with caller + rendezvous_key) are debug! under that
# target — they are the ONLY vantage that sees both halves of a rendezvous, so a
# bucket split vs an absent counterpart is told apart here, not in the peer.
# §4.5.1's AUTOMATIC half: when E2E_NODE_REFLECTION is set the node publishes
# that STUN URI in `advertise`, and the browsers are told NOTHING — they learn
# it by asking. That is the whole difference from E2E_ICE, which the user types.
NODE_REFLECT_ARGS=()
if [ -n "${E2E_NODE_REFLECTION:-}" ]; then
  NODE_REFLECT_ARGS=(--reflection-endpoint "$E2E_NODE_REFLECTION")
  echo "   node advertises its own reflector: $E2E_NODE_REFLECTION (§4.5.1)"
fi
RUST_LOG=info,entity_signaling=debug,entity_peer=debug nohup "$SIG" --ws-listen 0.0.0.0:$WSPORT --open "${NODE_REFLECT_ARGS[@]}" >/tmp/sig_repro.out 2>&1 &
sleep 2
NODE=$(grep -oE "peer_id:   [1-9A-HJ-NP-Za-km-z]+" /tmp/sig_repro.out | head -1 | awk '{print $2}')
echo "   node peer: $NODE"

echo ">> dist server on :$DISTPORT"
pkill -f "http.server $DISTPORT" 2>/dev/null || true
sleep 0.5
nohup python3 -m http.server $DISTPORT --directory dist >/tmp/dist_repro.log 2>&1 &
sleep 1

if [ "$TOPOLOGY" = "nat" ]; then
  # The NAT control runs HERE, not at topology bring-up: one of its properties
  # is "each peer reaches the host-served dist through its own NAT", and the
  # dist server is started a few lines above. Probing earlier tested an address
  # nothing was listening on and reported a broken NAT that was not broken.
  echo ">> NAT topology control (no direct path, two external addresses, host reachable)"
  bash "$SCRATCH/nat_topology.sh" probe || {
    echo "!! the NAT topology failed its own control — refusing to report a traversal"
    exit 1
  }
  # Hand the spike the reflector, through the SAME connector row a user types.
  export E2E_ICE="stun:$(cat /tmp/entity-rtc-stun-addr)"
  echo "   reflectors for the connector row: $E2E_ICE"
fi

echo ">> driving integration spike"
BEFORE=$(wc -l < /tmp/sig_repro.out)
# The spike is the GATE: `main()` returns 0 only on a BIDIRECTIONAL pass (both
# channels open AND both directions status=200), 1 otherwise. Capture that rc —
# do NOT `|| true` it away, or the rig always exits 0 and can never fail a gate.
# `set +e` around it so a FAIL still prints the node-vantage diagnostics below
# (the whole debugging value) instead of `set -e` aborting before them.
# Spike is configurable so this same infra (bridge + 2 firefox + node + dist)
# drives either the rung-1 exec gate (default) or the chat-over-webrtc flow
# (SPIKE=spike_chat_over_webrtc.py SPIKE_ARGS=""). arg1 is always the node peer.
SPIKE="${SPIKE:-spike_rung1_integration.py}"
SPIKE_ARGS="${SPIKE_ARGS-get 30}"
set +e
echo ">> spike: $SPIKE $NODE $SPIKE_ARGS"
# shellcheck disable=SC2086
SPIKE_OUT=/tmp/spike_out_$$.txt
python3 "$SCRATCH/$SPIKE" "$NODE" $SPIKE_ARGS 2>&1 | tee "$SPIKE_OUT"
DRIVE_RC=${PIPESTATUS[0]}
set -e
echo ""
echo ">> NODE VANTAGE — signaling offer/collect by (caller, rendezvous_key) during run:"
echo "   (the discriminator, per ROUTING-2026-08-04-the-establisher-was-swallowing-...)"
# The node logs via tracing's pretty formatter → ANSI colour codes AND a `k=v`
# (not `k =v`) field style, so a naive grep misses everything. Strip ANSI first.
NODELOG=$(tail -n +$((BEFORE+1)) /tmp/sig_repro.out | sed $'s/\x1b\\[[0-9;]*m//g')
echo "   -- OFFER deposits by (caller, key) --"
echo "$NODELOG" | grep "signaling offer: deposit" \
  | grep -oE 'caller="[^"]+" rendezvous_key=RendezvousKey\([0-9a-f]+\)' | sort | uniq -c

# SIGNALING §11.5 teeth (NETWORK §10.3 obligation 5) — the single-flight
# assertion. A channel opening is NOT sufficient: the gate MUST also fail if it
# opened only by BRUTE FORCE — many independent negotiations deposited until two
# happened to overlap. Pre-single-flight core-rust deposited ~470 OFFERs/side;
# per-peer single-flight coalescing collapses that to a handful (one sustained
# negotiation's dedup-by-hash retrickle).
#
# COUNTING SEMANTICS (arch-pinned, §11.5 @ 78fdd13): offer deposits / node
# vantage / per side / per establishment. The conformant figure is ~4/side, NOT
# §7.2's exchange budget of 3 — §7.2 bounds exchange *attempts*, and each of the
# ≤3 exchanges deposits an offer PLUS a bounded glare-rollback / ICE-restart
# re-offer (§6.5), so a clean establishment lands a handful of deposits, not 3.
# The property that fails brute force is that the ceiling is a FIXED O(1), not
# its exact value. We set it to 8 = §7.2's 3 + re-offer headroom (above the
# theoretical conformant max ~6, well below ~470), tight enough to also catch a
# subtler 2x over-deposit regression that a looser 16 would wave through.
# Retune via SIG_DEPOSIT_BOUND if a real-node S5 run shows higher conformant
# glare. A volume-blind gate would pass a brute-force regression green — forbidden.
SIG_DEPOSIT_BOUND="${SIG_DEPOSIT_BOUND:-8}"
MAX_DEPOSITS=$(echo "$NODELOG" | grep "signaling offer: deposit" \
  | grep -oE 'caller="[^"]+" rendezvous_key=RendezvousKey\([0-9a-f]+\)' \
  | sort | uniq -c | awk '{print $1}' | sort -rn | head -1)
MAX_DEPOSITS="${MAX_DEPOSITS:-0}"
echo "   -- §11.5 deposit bound: max ${MAX_DEPOSITS}/side (O(1) bound ${SIG_DEPOSIT_BOUND}) --"
if [ "$TOPOLOGY" = "split" ]; then
  # NOT APPLIED HERE, and not because it is inconvenient. §11.5's bound is a
  # property of a **completing** establishment: "one negotiation, not many."
  # In the split rig establishment can never complete by construction, so the
  # establisher keeps re-offering for the whole run and the per-(caller,key)
  # counter accumulates retries across every attempt. Applying a
  # single-establishment ceiling to an unbounded-retry scenario measures the
  # length of the run, not single-flight conformance.
  #
  # Reported loudly rather than dropped, because the NUMBER is itself a
  # finding worth routing: a pair that can never connect deposits at this rate
  # into a shared rendezvous bucket, indefinitely. §11.2 SHOULDs "retry with a
  # fresh nonce, up to a small bounded count, before abandoning to a fallback";
  # whether this path honors that — and what a node operator sees when many
  # such pairs exist — is a real question this rig can now ask.
  echo "   ⓘ  §11.5 bound NOT applied in split topology (see the note in this"
  echo "      script): establishment cannot complete, so this counts retries"
  echo "      over the run, not one establishment. Measured rate is the finding:"
  echo "      ${MAX_DEPOSITS} deposits/side against a permanently unreachable peer."

  # --- §13 item 6: WHICH retry shape --------------------------------------
  # The node cannot tell "one unbounded retry" from "many bounded ones", because
  # deposits key on (caller, rendezvous_key) and every retry reuses the key. The
  # PEER can: each failed `establish_live` logs exactly one "negotiation to …
  # failed" line, so that count is completed negotiations, and deposits ÷
  # negotiations is the per-negotiation figure §11.5 actually bounds.
  NEG=$(grep -oE 'NEGOTIATION_ATTEMPTS=[0-9]+' "$SPIKE_OUT" 2>/dev/null \
        | cut -d= -f2 | sort -rn | head -1)
  NEG="${NEG:-0}"
  echo "   -- §13 item 6: retry shape --"
  if [ "$NEG" -gt 0 ]; then
    PER=$(awk -v d="$MAX_DEPOSITS" -v n="$NEG" 'BEGIN{printf "%.1f", d/n}')
    echo "      completed negotiations (peer vantage) : ${NEG}"
    echo "      deposits / negotiation                : ${PER}"
    echo "      => MANY BOUNDED negotiations, re-triggered by the caller —"
    echo "         NOT one unbounded retry. The establisher runs ONE negotiation"
    echo "         and never retries it (main_thread_establish.rs, §7.2.1"
    echo "         caller_owns_retry); the repetition is the caller's."
  else
    echo "      no negotiation-failure lines found — cannot classify this run."
    echo "      (Expected >0 in split topology; if 0, the establisher never ran"
    echo "      and the deposit count means something else entirely.)"
  fi
elif [ "$MAX_DEPOSITS" -gt "$SIG_DEPOSIT_BOUND" ]; then
  echo "   ❌ §11.5 FAIL: ${MAX_DEPOSITS} OFFER deposits/side exceeds the O(1) bound"
  echo "      ${SIG_DEPOSIT_BOUND} — establishment is brute-force (obligation-5 single-flight"
  echo "      regression) even though a channel may have opened. See ROUTING-2026-08-06."
  DRIVE_RC=1
fi

echo "   -- COLLECT reads by (caller, key, included_count) --"
echo "$NODELOG" | grep "signaling collect" \
  | grep -oE 'caller="[^"]+" rendezvous_key=RendezvousKey\([0-9a-f]+\) included_count=[0-9]+' | sort | uniq -c
echo ""
echo "   READ (offer lines are the tell):"
echo "   · no offer lines at all        → nobody deposited (offerer rule sent both to suppress = ids disagree)"
echo "   · two offers, DIFFERENT keys    → bucket split; send the two keys+callers to core-rust"
echo "   · two offers, SAME key, collect included_count=0 → node not returning what it holds (core-rust's)"
echo "   · collect non-zero, no channel  → past rendezvous; read the peer-side §6.5 warn! (spike output above)"
echo ""
echo "(node peer id in \$NODE=$NODE; teardown with: bash $0 teardown)"

# The gate's verdict IS this script's exit code. Containers/node/dist are left
# up on purpose (manual inspection); `make e2e-webrtc` tears them down around
# this run. A bare `bash rung1_repro.sh` now exits non-zero on a FAIL.
echo ""
echo ">> gate exit: $DRIVE_RC ($([ "$DRIVE_RC" -eq 0 ] && echo 'PASS ✅' || echo 'FAIL ❌'))"
exit "$DRIVE_RC"

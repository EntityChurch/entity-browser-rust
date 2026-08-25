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
NET=entity-rtc-spike
CORE=../entity-core-rust
SIG="$CORE/target/debug/entity-signaling-node"
DISTPORT=8092
WSPORT=4071
SCRATCH="$(dirname "$0")"

teardown() {
  echo ">> teardown"
  podman rm -f rtc-a rtc-b >/dev/null 2>&1 || true
  podman network rm "$NET" >/dev/null 2>&1 || true
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
  echo ">> rebuilding signaling node from core-rust HEAD ($CORE_SHA) to match dist/"
  ( cd "$CORE" && cargo build -p entity-signaling-node 2>&1 | tail -2 )
fi
echo "── build posture ─────────────────────────────────────────────"
echo "   core-rust HEAD : $CORE_SHA${CORE_DIRTY:+  (working tree DIRTY — node/dist may disagree)}"
echo "   §6.5 is Require: a VerificationUnavailable on the browser leg = MIXED"
echo "   BUILDS, not NAT. dist/ (make wasm) and the node above MUST come from"
echo "   this same core-rust checkout. To debug rung-1 under the tolerant"
echo "   posture, revert Require upstream (one line) and rebuild both."
echo "──────────────────────────────────────────────────────────────"

echo ">> shared bridge network"
podman network exists "$NET" || podman network create "$NET" >/dev/null

echo ">> two firefox containers on the bridge (distinct routable IPs)"
podman container exists rtc-a || podman run -d --rm --name rtc-a --network "$NET" -p 4446:4444 "$IMG" >/dev/null
podman container exists rtc-b || podman run -d --rm --name rtc-b --network "$NET" -p 4447:4444 "$IMG" >/dev/null
for p in 4446 4447; do
  for i in $(seq 1 30); do curl -s -m2 localhost:$p/status 2>/dev/null | grep -q '"ready": *true' && break; sleep 1; done
done
echo "   rtc-a=$(podman inspect rtc-a --format '{{json .NetworkSettings.Networks}}' | grep -oE '"IPAddress":"[^"]+"' | head -1)"
echo "   rtc-b=$(podman inspect rtc-b --format '{{json .NetworkSettings.Networks}}' | grep -oE '"IPAddress":"[^"]+"' | head -1)"

echo ">> signaling node (debug) on :$WSPORT"
pkill -f "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT" 2>/dev/null || true
sleep 0.5
# entity_signaling=debug is load-bearing: the per-bucket `signaling offer:` /
# `signaling collect` lines (with caller + rendezvous_key) are debug! under that
# target — they are the ONLY vantage that sees both halves of a rendezvous, so a
# bucket split vs an absent counterpart is told apart here, not in the peer.
RUST_LOG=info,entity_signaling=debug,entity_peer=debug nohup "$SIG" --ws-listen 0.0.0.0:$WSPORT --open >/tmp/sig_repro.out 2>&1 &
sleep 2
NODE=$(grep -oE "peer_id:   [1-9A-HJ-NP-Za-km-z]+" /tmp/sig_repro.out | head -1 | awk '{print $2}')
echo "   node peer: $NODE"

echo ">> dist server on :$DISTPORT"
pkill -f "http.server $DISTPORT" 2>/dev/null || true
sleep 0.5
nohup python3 -m http.server $DISTPORT --directory dist >/tmp/dist_repro.log 2>&1 &
sleep 1

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
python3 "$SCRATCH/$SPIKE" "$NODE" $SPIKE_ARGS
DRIVE_RC=$?
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

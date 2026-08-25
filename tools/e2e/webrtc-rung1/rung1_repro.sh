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
SIG=../entity-core-rust/target/debug/entity-signaling-node
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
RUST_LOG=info,entity_peer=debug nohup "$SIG" --ws-listen 0.0.0.0:$WSPORT --open >/tmp/sig_repro.out 2>&1 &
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
python3 "$SCRATCH/spike_rung1_integration.py" "$NODE" list 30 || true
echo ""
echo ">> signaling collect results (included_count) during run:"
tail -n +$((BEFORE+1)) /tmp/sig_repro.out | grep -oE "operation=(offer|collect).*included_count=[0-9]+" | sort | uniq -c | head
echo ""
echo "(node peer id in \$NODE=$NODE; teardown with: bash $0 teardown)"

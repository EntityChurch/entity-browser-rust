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

# --- RTC_SLOT: which parallel copy of this rig you are ------------------------
# EVERY name and port below used to be a fixed literal, and this box has six
# worktrees. That is not a collision hazard, it is a DEMOLITION one: each
# `make e2e-webrtc-*` target opens with `rung1_repro.sh teardown`, which
# `podman rm -f`s the containers by name, removes the networks by name, pkills
# the node and the dist server, and `rm -f`s the node keypair. A second seat
# starting any WebRTC gate therefore destroys the first seat's in-flight run —
# and the victim sees a spike failure, i.e. it reads as a product defect.
#
# This is 487b100e's finding one rig over, in the worse direction: that commit
# closed "two seats share one port space" for `make e2e-worker` with a REFUSAL.
# The WebRTC rig is the entry point that fix did not enumerate, and it destroys
# where the other refuses.
#
# SLOT 0 IS BYTE-IDENTICAL TO EVERY INVOCATION BEFORE THIS EXISTED — same
# container names, same networks, same ports, same /tmp paths. Nothing that
# worked yesterday moves, and no muscle memory breaks. A non-zero slot suffixes
# every name and offsets every port by slot×10.
#
# ONE knob moves ALL of it, deliberately — the GRID_PORT precedent two hundred
# lines up in the Makefile: moving only the HTTP port left the ZeroMQ bus
# colliding and the container died with `Address already in use`, which reads as
# "the image is broken". Five ports and nine names here have the same property.
RTC_SLOT="${RTC_SLOT:-0}"
case "$RTC_SLOT" in
  ''|*[!0-9]*) echo "!! RTC_SLOT must be a non-negative integer (got '$RTC_SLOT')" >&2; exit 1 ;;
esac
# Bounded at 9 so the ×10 stride cannot walk a port into the next service's
# space: slot 9 puts the browsers at 4536/4537 and the node at 4161, both still
# clear of everything this repo binds.
[ "$RTC_SLOT" -gt 9 ] && { echo "!! RTC_SLOT must be 0-9 (got $RTC_SLOT)" >&2; exit 1; }
SFX=""
[ "$RTC_SLOT" -ne 0 ] && SFX="-$RTC_SLOT"
STRIDE=$(( RTC_SLOT * 10 ))

NET=entity-rtc-spike$SFX
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
NET_A=entity-rtc-nat-a$SFX
NET_B=entity-rtc-nat-b$SFX
CTR_A=rtc-a$SFX
CTR_B=rtc-b$SFX
CORE=../entity-core-rust
# Overridable. On a host with no `cargo` the build step below uses
# `make e2e-signaling-node` (inside the image) and points this at its output.
SIG="${SIG:-$CORE/target/debug/entity-signaling-node}"
DISTPORT=$(( 8092 + STRIDE ))
WSPORT=$(( 4071 + STRIDE ))
PORT_A=$(( 4446 + STRIDE ))
PORT_B=$(( 4447 + STRIDE ))
SCRATCH="$(dirname "$0")"
# The node vantage and the dist server's log. Per-slot for the same reason the
# containers are: two rigs appending to one file makes the deposit counting
# below read another seat's negotiations as this run's.
NODE_LOG=/tmp/sig_repro$SFX.out
DIST_LOG=/tmp/dist_repro$SFX.log
# nat_topology.sh builds its own containers and networks and must land in the
# same slot, or a `TOPOLOGY=nat` run tears down the shared rig's peers by name.
export RTC_SLOT SFX STRIDE CTR_A CTR_B
# The spikes hardcoded these five literals. They read the environment now, with
# today's values as the defaults, so a bare `python3 spike_*.py` still works.
export RTC_A_BASE="http://localhost:$PORT_A"
export RTC_B_BASE="http://localhost:$PORT_B"
export RTC_APP="http://host.containers.internal:$DISTPORT"
export RTC_NODE_WS="ws://host.containers.internal:$WSPORT"
export RTC_NODE_LOG="$NODE_LOG"
# A STABLE NODE IDENTITY, so a restart is a restart and not a different node.
# Ephemeral is the node's default and is right for a stateless introducer (§1.3:
# losing a node drops in-flight handshakes and loses nothing that mattered) —
# but it makes the `node-restart` variant below unable to ask its own question.
# With a fresh keypair the browsers would be dialling a peer id that no longer
# exists, so a failure to reconnect is explained by the identity change and says
# nothing about whether the carrier noticed its connection had died. Same file
# across every invocation, including the separate `node-restart` one.
NODE_KEYPAIR="${NODE_KEYPAIR:-/tmp/entity-rung1-node$SFX.key}"

# --- The occupancy lock -------------------------------------------------------
# WHY A PID AND NOT "ARE THE CONTAINERS UP": a FAILED run deliberately leaves its
# containers, node and dist server running for manual inspection (see the note at
# the foot of this file). So "rtc-a exists" cannot mean "a run is in flight" —
# refusing on leftovers would refuse every run after a failure, which is exactly
# how a guard gets switched off. A live PID and a corpse are different facts and
# the lock keeps them apart: live → refuse, dead → say so and take it over.
# RTC_LOCK_DIR is a TEST AFFORDANCE and nothing in a gate sets it — same shape
# and same reason as `?bootstall=`. It lets tools/webrtc-slot-check.sh falsify
# the occupancy guard in a temp dir instead of writing a fake owner into a slot
# somebody may be using, which would be this bug wearing the fix's clothes.
LOCK="${RTC_LOCK_DIR:-/tmp}/entity-rung1-rig$SFX.lock"
lock_owner_alive() {
  [ -f "$LOCK" ] || return 1
  local pid; pid=$(cat "$LOCK" 2>/dev/null)
  case "$pid" in ''|*[!0-9]*) return 1 ;; esac
  # Our own PID never counts as an occupant — the `teardown` the make targets
  # run *around* a gate is a different process, but a re-entrant call inside one
  # run must not deadlock against itself.
  [ "$pid" = "$$" ] && return 1
  kill -0 "$pid" 2>/dev/null
}

# --- Whose process is that? ---------------------------------------------------
# The two lines below used to be `pkill -f "http.server $DISTPORT"` and the same
# for the node. A pattern sweep answers "is something on this port", never "is it
# OURS" — and `make e2e-worker` defaults to :8092, the SAME port this rig serves
# dist on at slot 0. So this rig's teardown terminated another seat's e2e-worker
# staging server. Measured 2026-09-17: while verifying the slot fix, a live
# `make e2e-worker` in the sibling worktree had a child
# `python3 -m http.server 8092 --directory dist`, and `http.server 8092` matches
# it exactly. The fix for cross-seat demolition reproduced cross-GATE demolition
# one port over, which is the same sentence 487b100e was written about.
#
# So: record what we start, and stop only that — re-checking the live cmdline
# first, because a pid is reused and a stale pidfile pointing at somebody else's
# process is the same bug with a smaller window.
PIDFILE="${RTC_LOCK_DIR:-/tmp}/entity-rung1-procs$SFX.pids"
record_pid() { printf '%s %s\n' "$1" "$2" >> "$PIDFILE"; }
stop_ours() {
  local tag="$1" marker="$2" t pid cmd
  [ -f "$PIDFILE" ] || return 0
  while read -r t pid; do
    [ "$t" = "$tag" ] || continue
    [ -r "/proc/$pid/cmdline" ] || continue
    cmd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)
    case "$cmd" in *"$marker"*) kill "$pid" 2>/dev/null || true ;; esac
  done < "$PIDFILE"
}
# Report — never sweep — a stranger on our port. Refusing would block a rig whose
# previous run predates the pidfile; sweeping is the bug. Naming it is what turns
# "the gate is behaving strangely" into one line.
warn_foreign_holder() {
  local port="$1" what="$2"
  command -v ss >/dev/null 2>&1 || return 0
  ss -ltn "sport = :$port" 2>/dev/null | grep -q LISTEN || return 0
  echo "   ⓘ  :$port ($what) is held by a process this rig did not start."
  echo "      Left alone on purpose — it may be another seat's gate. If it is a"
  echo "      leftover of yours, stop it by hand, or use another slot: RTC_SLOT=1"
}

teardown() {
  echo ">> teardown (slot $RTC_SLOT)"
  podman rm -f "$CTR_A" "$CTR_B" >/dev/null 2>&1 || true
  podman network rm "$NET" >/dev/null 2>&1 || true
  podman network rm "$NET_A" "$NET_B" >/dev/null 2>&1 || true
  bash "$(dirname "$0")/nat_topology.sh" down >/dev/null 2>&1 || true
  stop_ours node "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT"
  stop_ours dist "http.server $DISTPORT"
  rm -f "$PIDFILE"
  sleep 0.3
  warn_foreign_holder "$WSPORT" "signaling node"
  warn_foreign_holder "$DISTPORT" "dist server"
  echo ">> done"
}
# `slot` — print everything this slot owns, and whether anyone is in it.
# Two jobs: an operator asking "what is on 4456 / who holds slot 0", and the
# lint-time check that slot N and slot M share nothing. That check reads THIS
# output rather than re-deriving the names, so it cannot drift from the rig —
# a second expression of the derivation is the thing it would be testing.
if [ "${1:-}" = "slot" ]; then
  echo "slot=$RTC_SLOT"
  for kv in "net=$NET" "net_a=$NET_A" "net_b=$NET_B" \
            "ctr_a=$CTR_A" "ctr_b=$CTR_B" \
            "port_a=$PORT_A" "port_b=$PORT_B" "wsport=$WSPORT" "distport=$DISTPORT" \
            "node_log=$NODE_LOG" "dist_log=$DIST_LOG" "keypair=$NODE_KEYPAIR" \
            "lock=$LOCK"; do echo "$kv"; done
  if lock_owner_alive; then
    echo "occupied=yes pid=$(cat "$LOCK")"
  elif [ -f "$LOCK" ]; then
    echo "occupied=stale pid=$(cat "$LOCK")"
  else
    echo "occupied=no"
  fi
  exit 0
fi

if [ "${1:-}" = "teardown" ]; then
  # A teardown is as destructive as a run and gets the same guard. This is the
  # line that mattered: every make target opens with one, so an unguarded
  # teardown is the demolition even when the run that follows never starts.
  if lock_owner_alive; then
    echo "!! REFUSING to tear down slot $RTC_SLOT — pid $(cat "$LOCK") is running a gate there."
    echo "   Tearing it down would destroy an in-flight run, and the victim would"
    echo "   see a spike failure, i.e. it would read as a product defect."
    echo "   Run your gate in another slot:  make <target> RTC_SLOT=1"
    exit 1
  fi
  teardown
  rm -f "$NODE_KEYPAIR" "$LOCK"
  exit 0
fi

# Bring the signaling node up on :$WSPORT with a STABLE identity, appending to
# the same log the diagnostics read. Shared by the main flow and `node-restart`.
start_node() {
	local before
	# NOT `$(grep -c ... || echo 0)`: `grep -c` PRINTS 0 and RETURNS 1 when it
	# matches nothing, so the fallback fires too and the substitution is the
	# two-line string "0\n0", which `[` then refuses as a non-integer. The
	# failure surfaced as "the node did not announce itself", i.e. as the
	# condition this function exists to detect.
	before=$(grep -c "peer_id:" "$NODE_LOG" 2>/dev/null)
	before=${before:-0}
	local reflect=()
	[ -n "${E2E_NODE_REFLECTION:-}" ] && reflect=(--reflection-endpoint "$E2E_NODE_REFLECTION")
	# APPEND (`>>`), never truncate: the node vantage is the only place that sees
	# both halves of a rendezvous, and a restart that wiped the log would destroy
	# the evidence for everything that happened before it — including the
	# deposits the restart is supposed to be measured against.
	RUST_LOG=info,entity_signaling=debug,entity_peer=debug nohup "$SIG" \
		--ws-listen 0.0.0.0:$WSPORT --open --keypair "$NODE_KEYPAIR" "${reflect[@]}" \
		>>"$NODE_LOG" 2>&1 &
	record_pid node $!
	# Wait for THIS start's banner rather than sleeping: a fixed sleep makes a
	# slow start look like a node that never came up, and on the restart path
	# that would be reported as the defect under test.
	for _ in $(seq 1 40); do
		local now
		now=$(grep -c "peer_id:" "$NODE_LOG" 2>/dev/null)
		[ "${now:-0}" -gt "$before" ] && return 0
		sleep 0.25
	done
	echo "!! signaling node did not announce itself within 10s"
	return 1
}

# `node-restart` — kill the node and bring it back at the SAME identity, for
# H2's discriminator (*the carrier holds a dead connection to the node forever*).
# Invoked BY THE SPIKE mid-run, because only the spike knows when the browsers
# have finished their first exchange; node lifecycle stays here, where `$SIG`,
# `$WSPORT` and the keypair are resolved.
#
# ⚠ It asserts the identity is UNCHANGED before returning. A restart that
# silently minted a new peer id would make every downstream assertion measure
# the wrong thing, and it is the one failure this subcommand can have that looks
# exactly like the defect it exists to test.
if [ "${1:-}" = "node-restart" ]; then
	was=$(grep -oE "peer_id:   [1-9A-HJ-NP-Za-km-z]+" "$NODE_LOG" | tail -1 | awk '{print $2}')
	[ -x "$SIG" ] || SIG="$PWD/target/e2e-node/debug/entity-signaling-node"
	echo ">> node-restart: stopping the node at :$WSPORT (was $was)"
	# Ours only — this runs mid-gate, invoked by the spike, and a pattern sweep
	# here would reach a node another slot or another seat is depending on.
	stop_ours node "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT"
	sleep 1
	start_node || exit 1
	now=$(grep -oE "peer_id:   [1-9A-HJ-NP-Za-km-z]+" "$NODE_LOG" | tail -1 | awk '{print $2}')
	if [ "$was" != "$now" ]; then
		echo "!! node identity CHANGED across the restart ($was -> $now)."
		echo "   Every assertion after this would be about a different node. Is"
		echo "   --keypair $NODE_KEYPAIR writable?"
		exit 1
	fi
	echo ">> node-restart: back up, same identity ($now)"
	exit 0
fi

# --- Claim the slot -----------------------------------------------------------
# Below this line the script starts destroying and creating things, so this is
# where occupancy stops being advisory. Note it sits AFTER the `node-restart`
# subcommand, which is invoked BY THE SPIKE from inside a live run and must not
# refuse itself out of its own gate.
if lock_owner_alive; then
  echo "!! RTC SLOT $RTC_SLOT IS OCCUPIED — pid $(cat "$LOCK") is running a gate there."
  echo "   Starting here would tear down its containers, kill its node and its"
  echo "   dist server mid-run. The victim would report a spike failure, which"
  echo "   reads as a product defect rather than as a rig collision."
  echo ""
  echo "   Run in another slot:   make <target> RTC_SLOT=1"
  echo "   (slots 0-9; each gets its own containers, networks, ports and /tmp)"
  echo "   If you are certain that pid is gone:   rm -f $LOCK"
  exit 1
fi
if [ -f "$LOCK" ]; then
  echo ">> slot $RTC_SLOT: taking over a STALE lock (pid $(cat "$LOCK") is gone)"
fi
echo $$ > "$LOCK"
# Release on ANY exit, including the `set -e` aborts and the FAIL path — a lock
# that outlives its run turns the guard into a permanent refusal, and a guard
# people have to clear by hand is one they learn to delete. The containers are
# deliberately LEFT UP on failure for inspection; the lock is not part of that.
trap 'rm -f "$LOCK"' EXIT

# --- Port preflight: fail in 1s, not after 30s of containers ------------------
# ORDER IS THE WHOLE THING. Clear OUR OWN leftovers first (a previous run of this
# slot is deliberately left up for inspection, so its node and dist are normal
# and must not be mistaken for a stranger), THEN refuse whatever is left, because
# whatever is left is by definition not ours.
#
# Checked here rather than at the two start sites so a busy port costs a second
# instead of two container pulls, a network and a node — the first cut refused at
# the dist step, ~40s in, having already built everything it was about to discard.
stop_ours node "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT"
stop_ours dist "http.server $DISTPORT"
sleep 0.4
for probe in "$WSPORT:signaling node" "$DISTPORT:dist server"; do
  pport=${probe%%:*}; pwhat=${probe#*:}
  if ss -ltn "sport = :$pport" 2>/dev/null | grep -q LISTEN; then
    echo "!! :$pport (slot $RTC_SLOT's $pwhat) is held by a process this rig did not start."
    echo "   Refusing, rather than binding over it or sweeping it away — :8092 is"
    echo "   also \`make e2e-worker\`'s default, so this is as often that suite as"
    echo "   it is another WebRTC run, and sweeping the port is how this rig used"
    echo "   to take other seats down."
    echo ""
    echo "   Run in another slot:   make <target> RTC_SLOT=1"
    exit 1
  fi
done

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
  podman container exists "$CTR_A" || podman run -d --rm --name "$CTR_A" --network "$NET_A" -p $PORT_A:4444 "$IMG_A" >/dev/null
  podman container exists "$CTR_B" || podman run -d --rm --name "$CTR_B" --network "$NET_B" -p $PORT_B:4444 "$IMG_B" >/dev/null
else
  echo ">> shared bridge network"
  podman network exists "$NET" || podman network create "$NET" >/dev/null
  echo ">> two browser containers on the bridge (distinct routable IPs): A=$ENGINE_A B=$ENGINE_B"
  # `--shm-size=2g`: Chromium's renderer dies on the 64 MiB default /dev/shm and
  # presents as a session that starts and then goes away mid-run. Harmless to
  # Firefox, so it is unconditional rather than a branch that only the mixed rig
  # exercises.
  podman container exists "$CTR_A" || podman run -d --rm --name "$CTR_A" --shm-size=2g --network "$NET" -p $PORT_A:4444 "$IMG_A" >/dev/null
  podman container exists "$CTR_B" || podman run -d --rm --name "$CTR_B" --shm-size=2g --network "$NET" -p $PORT_B:4444 "$IMG_B" >/dev/null
fi
for p in $PORT_A $PORT_B; do
  for i in $(seq 1 30); do curl -s -m2 localhost:$p/status 2>/dev/null | grep -q '"ready": *true' && break; sleep 1; done
done
A_IP=$(podman inspect "$CTR_A" --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
B_IP=$(podman inspect "$CTR_B" --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
echo "   $CTR_A=$A_IP   $CTR_B=$B_IP   (slot $RTC_SLOT: grids $PORT_A/$PORT_B, node :$WSPORT, dist :$DISTPORT)"

# --- The control has to be controlled ----------------------------------------
# Assert the topology we THINK we built. A split rig whose isolation silently
# failed is worse than no rig: it would "prove" host candidates traverse NATs.
# So probe the actual path A->B and fail loudly if it disagrees with TOPOLOGY.
# (Shared is probed too — if the bridge ever stopped being mutually routable,
# every green run of the positive gate would have been measuring nothing.)
probe_a_to_b() {
  podman exec "$CTR_A" timeout 8 curl -s -m 5 -o /dev/null -w '%{http_code}' \
    "http://$B_IP:4444/status" 2>/dev/null || true
}
echo ">> probing the A->B path (the rig's own control)"
PROBE=$(probe_a_to_b)
if [ "$TOPOLOGY" = "nat" ]; then
  : # already probed above; probe_a_to_b is informational here
elif [ "$TOPOLOGY" = "split" ]; then
  if [ "$PROBE" = "200" ]; then
    echo "!! ISOLATION LEAKED: $CTR_A reached $CTR_B at $B_IP (HTTP 200)."
    echo "   The split rig would measure nothing — a media path exists that a"
    echo "   NAT'd pair would not have. Check 'podman network inspect $NET_A'"
    echo "   for isolate=true, and that no other network joins both containers."
    exit 1
  fi
  echo "   A->B blocked (curl said '${PROBE:-timeout}') — no direct path, as required"
else
  if [ "$PROBE" != "200" ]; then
    echo "!! $CTR_A could NOT reach $CTR_B at $B_IP on the shared bridge (got '${PROBE:-timeout}')."
    echo "   The positive rig assumes mutually routable host candidates; without"
    echo "   that this run proves nothing about the §6.5 mechanism."
    exit 1
  fi
  echo "   A->B reachable (200) — host candidates are mutually routable, as required"
fi

echo ">> signaling node (debug) on :$WSPORT"
# Clear OUR previous run's node off the port, never a stranger's. If somebody
# else holds it, `start_node` fails to bind and says so — which is the honest
# outcome; sweeping the port is how this rig used to take other seats down.
stop_ours node "entity-signaling-node --ws-listen 0.0.0.0:$WSPORT"
sleep 0.5
# entity_signaling=debug is load-bearing: the per-bucket `signaling offer:` /
# `signaling collect` lines (with caller + rendezvous_key) are debug! under that
# target — they are the ONLY vantage that sees both halves of a rendezvous, so a
# bucket split vs an absent counterpart is told apart here, not in the peer.
# §4.5.1's AUTOMATIC half: when E2E_NODE_REFLECTION is set the node publishes
# that STUN URI in `advertise`, and the browsers are told NOTHING — they learn
# it by asking. That is the whole difference from E2E_ICE, which the user types.
if [ -n "${E2E_NODE_REFLECTION:-}" ]; then
  echo "   node advertises its own reflector: $E2E_NODE_REFLECTION (§4.5.1)"
fi
: >"$NODE_LOG"
start_node || exit 1
NODE=$(grep -oE "peer_id:   [1-9A-HJ-NP-Za-km-z]+" "$NODE_LOG" | head -1 | awk '{print $2}')
echo "   node peer: $NODE  (identity from $NODE_KEYPAIR — stable across a restart)"

echo ">> dist server on :$DISTPORT"
# :8092 at slot 0 is ALSO `make e2e-worker`'s default. A pattern sweep here
# terminated that suite's staging server — measured against a live run. Ours only.
stop_ours dist "http.server $DISTPORT"
sleep 0.5
# REFUSE a port we did not clear, rather than binding over it or failing quietly.
# Dropping the pattern sweep means a stranger's server now survives — which is the
# point — but it also means python would fail to bind and this rig would serve the
# STRANGER'S bytes to both browsers. That is 487b100e's exact finding ("the loser's
# browser fetches the winner's bytes"), and the only reason it was not reachable
# here before is that the sweep was removing the evidence.
if ss -ltn "sport = :$DISTPORT" 2>/dev/null | grep -q LISTEN; then
  echo "!! :$DISTPORT is already served by a process this rig did not start."
  echo "   Both browsers would load ITS bytes, and every assertion below would be"
  echo "   about a build nobody here chose. Note :8092 is also \`make e2e-worker\`'s"
  echo "   default, so this is often that suite rather than another WebRTC run."
  echo ""
  echo "   Run in another slot:   make <target> RTC_SLOT=1"
  exit 1
fi
nohup python3 -m http.server $DISTPORT --directory dist >"$DIST_LOG" 2>&1 &
record_pid dist $!
sleep 1
# And confirm it is actually up — `record_pid` records what we spawned, not what
# survived (AP44's witness rule: assert the consequence, not the call).
if ! curl -s -m 3 -o /dev/null "http://localhost:$DISTPORT/index.html"; then
  echo "!! the dist server on :$DISTPORT did not come up — see $DIST_LOG"
  exit 1
fi

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
  export E2E_ICE="stun:$(cat "/tmp/entity-rtc-stun-addr$SFX")"
  echo "   reflectors for the connector row: $E2E_ICE"
fi

echo ">> driving integration spike"
BEFORE=$(wc -l < "$NODE_LOG")
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
NODELOG=$(tail -n +$((BEFORE+1)) "$NODE_LOG" | sed $'s/\x1b\\[[0-9;]*m//g')
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
#
# THREE STATES, AND THIS LINE USED TO FLATTEN THEM TO TWO — it printed
# `FAIL ❌` for anything non-zero, so a spike that had carefully reported
# `2 = INCONCLUSIVE` (the run could not put the mechanism at risk) was announced
# as a failure one line later, contradicting the `make` wrapper directly above
# it in the same output. *A three-state design is defeated at whatever boundary
# can only carry two*, and the boundary here was a ternary.
#
# ⚠ The code does NOT survive `make`: GNU make reports its own recipe failure as
# exit 2 whatever the recipe returned, which collides with INCONCLUSIVE. So a
# caller must read this LINE (or the `>>> <gate>: …` line make prints), never
# `$?`. Recorded in GOTCHAS under *Testing & the gates*.
case "$DRIVE_RC" in
	0) verdict='PASS ✅' ;;
	2) verdict='INCONCLUSIVE ⚠ — the run could not put the mechanism at risk' ;;
	*) verdict='FAIL ❌' ;;
esac
echo ""
echo ">> gate exit: $DRIVE_RC ($verdict)"
exit "$DRIVE_RC"

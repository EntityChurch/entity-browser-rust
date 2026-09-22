#!/usr/bin/env bash
# The WebRTC rig's slot isolation, asserted rather than claimed.
#
# WHY THIS EXISTS. Every `make e2e-webrtc-*` target opens with
# `rung1_repro.sh teardown`, and until RTC_SLOT landed that teardown removed
# containers by fixed name, removed three fixed networks, pkilled the node and
# the dist server by fixed port, and deleted a shared node keypair. This box has
# six worktrees. So a second seat starting any WebRTC gate did not COLLIDE with
# an in-flight run — it DEMOLISHED it, and the victim saw a spike failure, i.e.
# it read as a product defect. 487b100e closed the same hazard for
# `make e2e-worker` with a refusal; this rig was the entry point that fix did
# not enumerate, and it destroyed where the other refused.
#
# Four rows, and rows 1 and 2 are the ones that rot silently:
#   1. slot 0 is UNCHANGED — every name, port and path is the literal the rig
#      used before RTC_SLOT existed. A refactor that "tidied" the derivation
#      and moved slot 0 would strand every runbook, handoff and muscle memory.
#   2. two slots share NOTHING — pairwise disjoint across every value.
#   3. an OCCUPIED slot refuses a teardown.
#   4. a STALE lock does not refuse. This row is why the lock carries a pid at
#      all: a FAILED run deliberately leaves its containers up for inspection,
#      so "the containers exist" cannot mean "a run is in flight", and a guard
#      that refused on leftovers would refuse every run after a failure — which
#      is how a guard gets switched off rather than fixed.
#
# It reads the rig's own `slot` output rather than re-deriving the names. A
# second expression of the derivation is exactly the thing this would be
# testing (C15), and it would agree with itself while both drifted.
#
# No podman, no grid, no network: pure derivation plus a lockfile. That is what
# lets it sit in `make lint`.
set -uo pipefail

RIG="$(dirname "$0")/e2e/webrtc-rung1/rung1_repro.sh"
[ -f "$RIG" ] || { echo "!! webrtc-slot-check: cannot find $RIG"; exit 1; }

fail=0
red() { echo "   ❌ $*"; fail=1; }
ok()  { echo "   ✅ $*"; }

slot_of() { RTC_SLOT="$1" RTC_LOCK_DIR="${2:-/tmp}" bash "$RIG" slot 2>&1; }

echo ">> webrtc-slot-check: the rig's slot isolation"

# --- Row 1: slot 0 is byte-identical to the pre-RTC_SLOT literals -------------
# Spelled as literals on purpose. Deriving the expectation from the script is
# how a census ends up asserting the author's memory (the D25 ownership-census
# lesson): the whole claim is "these specific historical values did not move",
# so the historical values have to be written down.
S0="$(slot_of 0)"
declare -a EXPECT=(
  "slot=0"
  "net=entity-rtc-spike"
  "net_a=entity-rtc-nat-a"
  "net_b=entity-rtc-nat-b"
  "ctr_a=rtc-a"
  "ctr_b=rtc-b"
  "port_a=4446"
  "port_b=4447"
  "wsport=4071"
  "distport=8092"
  "node_log=/tmp/sig_repro.out"
  "dist_log=/tmp/dist_repro.log"
  "keypair=/tmp/entity-rung1-node.key"
)
row1=0
for e in "${EXPECT[@]}"; do
  grep -qxF "$e" <<<"$S0" || { red "slot 0 moved: expected '$e', slot 0 says '$(grep "^${e%%=*}=" <<<"$S0")'"; row1=1; }
done
# Anti-vacuity: a `slot` subcommand that printed nothing (or errored) would
# satisfy nothing above only by way of the greps failing — but a future one that
# renamed the KEYS would fail every row for the wrong reason. Assert the shape.
n_keys=$(grep -cE '^[a-z_]+=' <<<"$S0")
[ "${n_keys:-0}" -ge 13 ] || { red "VACUOUS: 'slot' printed $n_keys keys, expected >= 13"; row1=1; }
[ "$row1" = 0 ] && ok "slot 0 is unchanged ($n_keys keys, every historical literal intact)"

# --- Row 2: slots are pairwise disjoint ---------------------------------------
# `slot=N` itself is excluded — it is the label, not a resource.
values_of() { slot_of "$1" | grep -vE '^(slot|occupied)=' | cut -d= -f2-; }
row2=0
for a in 0 1 2; do
  for b in 1 2 3; do
    [ "$a" -ge "$b" ] && continue
    shared=$(comm -12 <(values_of "$a" | sort -u) <(values_of "$b" | sort -u))
    if [ -n "$shared" ]; then
      red "slots $a and $b SHARE: $(tr '\n' ' ' <<<"$shared")"
      row2=1
    fi
  done
done
[ "$row2" = 0 ] && ok "slots 0-3 are pairwise disjoint (no shared name, port or path)"

# --- Rows 3+4: the occupancy guard --------------------------------------------
# In a temp dir, so this never writes a fake owner into a slot somebody is
# using. Doing that would be the very bug under test, wearing the fix's clothes.
TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

# `( trap - EXIT; exec sleep )` and not a bare `sleep &`: a background job in
# bash INHERITS the EXIT trap, so killing it runs `rm -rf "$TMPD"` in the child
# and takes the temp dir with it. That is not hypothetical — the first cut of
# this file did exactly that, and row 4 then went GREEN on a lock file that had
# never been written: "a stale lock does not refuse" measured "no lock does not
# refuse". Hence `lock_written` below, which is what caught it.
# The `>/dev/null 2>&1` is load-bearing too: `$(spawn_sleeper)` is a command
# substitution, and a background child that inherits its stdout keeps the pipe
# open, so the substitution blocks for the whole 300s instead of returning the
# pid. Two bash gotchas in three lines, both of which fail as a hang or a
# silent pass rather than an error.
spawn_sleeper() { ( trap - EXIT; exec sleep 300 ) >/dev/null 2>&1 & echo $!; }
lock_written() {
  [ -s "$1" ] || { red "VACUOUS: the lock file was never written ($1) — this row measured nothing"; return 1; }
}

LIVE=$(spawn_sleeper)
echo "$LIVE" > "$TMPD/entity-rung1-rig-9.lock"
lock_written "$TMPD/entity-rung1-rig-9.lock"
out=$(RTC_SLOT=9 RTC_LOCK_DIR="$TMPD" bash "$RIG" teardown 2>&1); rc=$?
kill "$LIVE" 2>/dev/null; wait "$LIVE" 2>/dev/null
if [ "$rc" = 0 ]; then
  red "an OCCUPIED slot was torn down anyway (rc=0) — this is the demolition bug"
elif ! grep -q "REFUSING" <<<"$out"; then
  red "an occupied slot failed, but not with the refusal message: $out"
else
  ok "an occupied slot refuses a teardown (rc=$rc) and names RTC_SLOT as the way out"
  grep -q "RTC_SLOT=" <<<"$out" || red "   ...but the refusal does not name RTC_SLOT, so it is not actionable"
fi

# A genuinely dead pid: started, killed, reaped. Not a number picked out of the
# air — pid reuse would make that row pass or fail by luck.
DEAD=$(spawn_sleeper)
kill "$DEAD" 2>/dev/null; wait "$DEAD" 2>/dev/null
echo "$DEAD" > "$TMPD/entity-rung1-rig-9.lock"
lock_written "$TMPD/entity-rung1-rig-9.lock" || fail=1
out=$(RTC_SLOT=9 RTC_LOCK_DIR="$TMPD" bash "$RIG" teardown 2>&1); rc=$?
if [ "$rc" != 0 ]; then
  red "a STALE lock refused (rc=$rc) — every run after a failure would be blocked: $out"
else
  ok "a stale lock does not refuse (rc=0) — leftovers and an in-flight run stay apart"
fi

echo ""
if [ "$fail" = 0 ]; then
  echo ">> webrtc-slot-check: OK"
else
  echo ">> webrtc-slot-check: FAILED"
fi
exit "$fail"

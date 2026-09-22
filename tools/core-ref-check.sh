#!/usr/bin/env bash
# Does the kernel we just checked out carry the work this app depends on?
#
# Run against a fresh `actions/checkout` of entity-core-rust, from any release
# leg, before anything compiles:
#
#     tools/core-ref-check.sh ../entity-core-rust
#
# ── WHY THIS EXISTS ──────────────────────────────────────────────────────────
#
# `CORE_RUST_REF` names a tag on entity-core-rust's PUBLIC master. Its own block
# in release.yml already records the failure mode it was written for: a pin that
# does not resolve, which fails at checkout, loudly, before anything compiles.
# That is the good failure and it needs no help from this script.
#
# This is the other one. Measured 2026-09-20: entity-core-rust's public master
# is 69 commits behind the kernel this app is developed against, and
# `reader_ended` / `connection_over` — the two fixes that closed the
# vanished-peer arc — appear in ZERO files there. A pin moved to that commit
# RESOLVES, COMPILES, PASSES EVERY GATE WE OWN, and ships a build in which a
# peer that vanishes is never noticed by its counterpart.
#
# So the dangerous pin is not the one that fails. It is the one somebody sets to
# make a red checkout go green.
#
# ── WHY NOT JUST LET THE COMPILER DO IT ──────────────────────────────────────
#
# The compiler covers symbols WE CALL, and it has: release.yml's superseded pin
# `302b7f4` died at compile on a missing `NegotiationReport`. It cannot cover
# behaviour we DEPEND ON and do not call. Both markers below are internal to
# `core/peer` — we never name them, our suite is green without them, and the
# product is wrong without them. That gap is the whole subject.
#
# ── ADDING A MARKER ──────────────────────────────────────────────────────────
#
# A marker earns its place by being (a) something this app's correctness rests
# on, (b) invisible to our compiler and our tests, and (c) greppable. Give it
# the sentence a release engineer needs at 2am: what breaks if it is absent.
# A marker nobody can act on is noise in the one place noise is expensive.
set -euo pipefail

KERNEL="${1:?usage: core-ref-check.sh <path-to-entity-core-rust-checkout>}"

if [ ! -d "$KERNEL" ]; then
    echo "core-ref-check: no kernel checkout at '$KERNEL'" >&2
    exit 2
fi

# A checkout with no core/peer is not a kernel, and grepping it would report
# every marker missing — a wrong answer dressed as a strong one. Refuse instead.
if [ ! -d "$KERNEL/core/peer" ]; then
    echo "core-ref-check: '$KERNEL' has no core/peer — that is not an entity-core-rust checkout." >&2
    exit 2
fi

# marker <symbol> <path-under-kernel> <what breaks without it>
MISSING=0
marker() {
    local sym="$1" path="$2" why="$3"
    if grep -rq --include='*.rs' -- "$sym" "$KERNEL/$path" 2>/dev/null; then
        printf '  ok       %-20s %s\n' "$sym" "$path"
    else
        printf '  MISSING  %-20s %s\n' "$sym" "$path"
        printf '           %s\n' "$why"
        MISSING=$((MISSING + 1))
    fi
}

echo "core-ref-check: $KERNEL"
if [ -d "$KERNEL/.git" ]; then
    echo "  at $(git -C "$KERNEL" rev-parse --short HEAD 2>/dev/null || echo '(unknown)')"
fi

marker reader_ended core/peer \
    "A WebRTC data channel that dies is reported by the reader but nothing acts on it, so the
           counterpart of a vanished peer renders Connected forever and every dispatch at it burns
           the full 30s request deadline."

marker connection_over core/peer \
    "The accepted side of a §6.5 link has no fast-fail either. One link has two handshake roles and
           peer ids are fresh per run, so without this the defect above presents as a 50/50 flake."

if [ "$MISSING" -ne 0 ]; then
    cat >&2 <<EOF

core-ref-check: FAILED — $MISSING marker(s) absent from this kernel.

This pin resolves and will compile. That is the problem: it is missing work this
app's behaviour depends on, and no gate downstream of here can see it.

Do not pin around this. Either the kernel cuts a tag carrying the work, or the
release waits for one.
EOF
    exit 1
fi

echo "core-ref-check: ok — every marker present."

#!/usr/bin/env bash
# build-pair — THE one expression of "which two commits is this build made of".
#
# A build of this crate is identified by a PAIR, not by our commit. We link
# `entity-core-rust` by PATH DEPENDENCY across twenty paths under `bindings/`,
# `core/` and `extensions/`, and there is NO cross-repo lockfile — so the bundle
# hash is a function of OUR commit *and* whatever sibling checkout happens to be
# on disk. Measured 2026-09-05: a COMMENT-ONLY edit to `assets/sw.js` appeared to
# move the build id, which our own C9 rule says is impossible. Two rebuilds at a
# fixed commit returned the same new id, which separated "the build is
# non-deterministic" from "an input nobody was tracking changed". It was the
# sibling.
#
# **Why this is a file and not two lines inlined twice.** `build-stamp.sh`
# already computed this pair to write it into `index.html`, and a `site-dist`
# guard needs the identical answer. Two expressions of one rule is C15's defect
# verbatim — the four disagreeing cache-immutability rules, whose whole cost was
# that each passed its own tests. There is one here, and both callers read it.
#
# **Paths resolve from THIS SCRIPT's location, not from `cwd`.** The sibling sits
# beside the repo root in every environment that matters (the host checkout and
# the build image's bind mount alike), so `<script>/../..` is a stable answer
# where `../entity-core-rust` is a claim about who invoked us.
#
# Usage:
#   build-pair.sh                 → prints "<commit> <core_ref>" on one line
#   build-pair.sh --check         → exit 1 if the pair is not release-grade
#
# `--check` refuses three things and each has its own message, because "we could
# not tell", "you have uncommitted work" and "you asked for a ref you are not on"
# are three different problems with three different fixes (AP40).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
sibling="$(cd "$root/.." 2>/dev/null && pwd)/entity-core-rust"

# Same failure-is-not-a-build-failure rule `build-stamp.sh` states: git may be
# absent, the tree may not be a checkout, or the bind mount may trip git's
# ownership check. Every one of those is "we do not know", never a hard stop —
# the DEFAULT mode must stay usable in a build image. `--check` is where a
# release decides that "we do not know" is not good enough.
# **`git` WALKS UP, AND THAT MADE "not a checkout" REPORT A NEIGHBOUR'S COMMIT
# INSTEAD OF `unknown` — measured 2026-09-09, and it is a confident wrong answer,
# which is the one outcome the paragraph above promises this cannot be.**
# `git -C <dir> rev-parse HEAD` searches ANCESTOR directories for a repository.
# The parent meta dir `<shared-parent>` IS one, so a sibling path that is not
# itself a checkout resolves to the META repo's HEAD — `0bc11b3` where the answer
# should have been `unknown`. Provenance that names the wrong repository's commit
# is worse than provenance that admits it does not know: nothing downstream can
# tell them apart, and the number looks plausible.
#
# So confirm the repository we found IS this directory, not something above it.
# `--show-toplevel` is the discriminator; `pwd -P` on both sides so a symlinked
# path cannot make an honest match read as a mismatch.
read_ref() {
    local dir="$1" ref top want
    want=$(cd "$dir" 2>/dev/null && pwd -P) || { printf 'unknown'; return; }
    top=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null || true)
    [ -n "$top" ] && top=$(cd "$top" 2>/dev/null && pwd -P)
    if [ -z "$top" ] || [ "$top" != "$want" ]; then
        printf 'unknown'
        return
    fi
    ref=$(git -C "$dir" rev-parse --short HEAD 2>/dev/null || echo unknown)
    if [ "$ref" != "unknown" ] && ! git -C "$dir" diff --quiet HEAD 2>/dev/null; then
        ref="$ref-dirty"
    fi
    printf '%s' "$ref"
}

commit=$(read_ref "$root")

# THE PIN, WHEN ONE WAS CONSTRUCTED — `ENTITY_CORE_PIN` carries the full sha of
# the commit `core-pin.sh` materialized, and the Makefile exports it into the
# container alongside the mount that puts that tree at the sibling path.
#
# **Without this branch the stamp SILENTLY DEGRADES at exactly the moment we
# pin.** `build-stamp.sh` runs INSIDE the build image and calls this script; a
# materialized pin is a `git archive` export, which has no `.git`, so `read_ref`
# would answer `unknown` and a pinned release would stamp WEAKER provenance than
# an unpinned one. The strongest build we can make would have described itself as
# the one we could not identify.
#
# It is never `-dirty`: an archive of a commit is that commit's bytes by
# construction. There is no working tree here to be dirty.
if [ -n "${ENTITY_CORE_PIN:-}" ]; then
    core_ref="${ENTITY_CORE_PIN:0:7}"
else
    core_ref=$(read_ref "$sibling")
fi

if [ "${1:-}" != "--check" ]; then
    printf '%s %s\n' "$commit" "$core_ref"
    exit 0
fi

# TWO failure kinds, tracked apart, and the reason is a defect this guard had
# for about ten minutes: `ALLOW_DIRTY=1` was suppressing a CORE_RUST_REF
# mismatch as well. A hatch named for one failure MUST NOT absorb another —
# "I know my tree is dirty" is not "I know I am on the wrong kernel", and the
# second is the exact drift this file exists to stop. AP36: a guard that skips
# work answers ONE question; check every consequence is downstream of it.
dirty_fail=0
pin_fail=0
say() { printf '  %s\n' "$1" >&2; }

printf 'build-pair: (%s, %s)\n' "$commit" "$core_ref" >&2

# (1) DIRTY — the expensive one. These bytes are not reproducible from any
#     commit, by anyone, ever. A stamp saying `-dirty` is honest and useless.
for pair in "this repo:$commit" "entity-core-rust:$core_ref"; do
    name=${pair%%:*}; val=${pair#*:}
    case "$val" in
        *-dirty)
            say "REFUSED: $name has uncommitted changes ($val)."
            say "         A release tree built from a dirty checkout names a commit whose"
            say "         bytes nobody can reproduce. Commit, stash, or set ALLOW_DIRTY=1."
            dirty_fail=1
            ;;
        unknown)
            say "REFUSED: $name could not be read as a git checkout."
            say "         The build would stamp 'unknown', which is honest and unusable as"
            say "         provenance. Set ALLOW_DIRTY=1 only if you know why it is missing."
            dirty_fail=1
            ;;
    esac
done

# (2) THE ACTUAL PIN, and it is opt-in. `CORE_RUST_REF` is CI's variable name on
#     purpose (`.github/workflows/release.yml`), so the local build path and the
#     workflow express one intent one way. We VERIFY and never check out: a
#     sibling repo's git is read-only from here, and a tool that moves another
#     tree's HEAD is how a seat loses work that was never its own.
#
#     CONSTRUCTION OUTRANKS VERIFICATION. When `core-pin.sh` has materialized the
#     ref and the Makefile has mounted it over the sibling path, there is nothing
#     left to verify: the build is not reading that checkout at all, so its state
#     — clean, dirty, mid-rebase, on another branch entirely — cannot reach the
#     artifact. This is the whole point of the pin, and re-running the on-disk
#     comparison here would refuse a build that is STRICTLY more reproducible
#     than any build this check was ever able to pass.
if [ -n "${ENTITY_CORE_PIN:-}" ]; then
    say "pinned by construction: entity-core-rust @ ${ENTITY_CORE_PIN:0:7} (built from an export, not the checkout)"
elif [ -n "${CORE_RUST_REF:-}" ]; then
    want=$(git -C "$sibling" rev-parse --verify "${CORE_RUST_REF}^{commit}" 2>/dev/null || true)
    have=$(git -C "$sibling" rev-parse --verify HEAD 2>/dev/null || true)
    if [ -z "$want" ]; then
        say "REFUSED: CORE_RUST_REF=$CORE_RUST_REF does not resolve in entity-core-rust."
        say "         Fetch it there first. This script never mutates a sibling checkout."
        pin_fail=1
    elif [ "$want" != "$have" ]; then
        say "REFUSED: CORE_RUST_REF=$CORE_RUST_REF is ${want:0:7}, but that tree is on ${have:0:7}."
        say "         Check it out there, or drop CORE_RUST_REF to build against what is on disk."
        pin_fail=1
    else
        say "pinned: entity-core-rust is at $CORE_RUST_REF (${have:0:7})"
    fi
fi

if [ "$dirty_fail" != 0 ] && [ "${ALLOW_DIRTY:-}" = "1" ]; then
    say ""
    say "ALLOW_DIRTY=1 — proceeding on the dirty tree. The artifact stamps what it found."
    dirty_fail=0
fi

# Deliberately NOT waivable by ALLOW_DIRTY. The way out of a pin mismatch is to
# drop CORE_RUST_REF, which is an explicit statement that you are building
# against whatever is on disk — the thing you were asking to be protected from.
if [ "$pin_fail" != 0 ]; then
    say ""
    say "The CORE_RUST_REF failure above is NOT waivable by ALLOW_DIRTY — unset"
    say "CORE_RUST_REF to build against the checkout as it stands."
fi

[ "$dirty_fail" = 0 ] && [ "$pin_fail" = 0 ]

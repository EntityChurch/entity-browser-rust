#!/usr/bin/env bash
# ecf-lint — entity `data` is CANONICAL ECF, and `ciborium::into_writer` is not.
#
# **Why this is a lint and not a comment.** The rule was learned once, on
# 2026-08, at exactly one call site — `registry_publish.rs`'s
# `http_poll_profile_entity`, whose comment says *"`to_ecf`, NOT
# `ciborium::into_writer` — and this is now load-bearing"*. It was written about
# that site's circumstance (a profile fetched by hash, where
# `verify_and_decode` canonically re-encodes before hashing) and generalised
# nowhere. Five other encoders went on emitting non-canonical bytes.
#
# On 2026-09-06 the kernel landed `ENTITY-CORE-PROTOCOL` 0.8.2.11's §6.3 `put`
# admission ladder — a peer now **validates** the carried `content_hash` and
# MUST NOT author one — and the same latent defect became fatal on **every L1
# write**: `make test` went to 1377/18 with every failure in a tree-write path,
# and four e2e gates went red on both arms. The measured mismatch:
#
#     ShellState keys, as we wrote them:  wd, history, draft
#     ECF canonical (length, then lexical): wd, draft, history
#
# `ciborium` faithfully preserves our order; `to_ecf` canonicalizes and the
# encoder gets no say. So the hash we computed over our bytes could never match
# the hash the peer computes over its re-encode → `400 hash_mismatch`.
#
# **This is the AP44 shape exactly**: a rule that needs the word *every* was
# written as "and also do X here", was correct the day it landed, and decayed on
# the first call site added by someone who did not have the whole set in their
# head. So it is structural now.
#
# What it checks
# --------------
#
# `ciborium::into_writer` anywhere under `src/`, against a baseline count.
# The chokepoint is `entity_ecf::to_ecf`, which is canonical by construction.
#
# **The baseline carries a COUNT, not names** — the same stated limit every
# ratchet in this repo has (see `net-lint.sh`): a SWAP passes. Delete a
# legitimate use, add an illegitimate one, and the count still matches. It is a
# floor on quantity, not an allowlist of sites. When you change one, re-read
# them rather than trusting the number.
#
# What is legitimately in the baseline: encoders that are NOT producing entity
# `data` — a test fixture deliberately building a hostile, non-canonical body
# (`named_site.rs`), and a test helper encoding input for a decoder
# (`sexpr.rs`). Neither becomes an `Entity`.
#
# Usage: ./tools/ecf-lint.sh [--update-baseline]
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE_FILE="tools/ecf-lint-baseline.txt"

# Count call sites only — a line mentioning the symbol in prose (this rule is
# cited in five comments, which is the point) must not be counted as a use.
count() {
  grep -rn "ciborium::into_writer(" src/ --include=*.rs 2>/dev/null \
    | grep -vE "^[^:]+:[0-9]+: *//" \
    | wc -l | tr -d ' '
}

ACTUAL="$(count)"

if [ "${1:-}" = "--update-baseline" ]; then
  echo "$ACTUAL" > "$BASELINE_FILE"
  echo "ecf-lint: baseline updated to $ACTUAL"
  exit 0
fi

if [ ! -f "$BASELINE_FILE" ]; then
  echo "ecf-lint: FAIL — no baseline at $BASELINE_FILE" >&2
  exit 1
fi
EXPECTED="$(tr -d ' \n' < "$BASELINE_FILE")"

if [ "$ACTUAL" -gt "$EXPECTED" ]; then
  echo "ecf-lint: FAIL — $ACTUAL uses of ciborium::into_writer in src/, baseline is $EXPECTED" >&2
  echo "" >&2
  echo "  Entity \`data\` must be CANONICAL ECF. Use \`entity_ecf::to_ecf(&value)\`," >&2
  echo "  not \`ciborium::into_writer\`: ciborium preserves YOUR map key order," >&2
  echo "  \`to_ecf\` canonicalizes (length, then lexical). If those differ, the" >&2
  echo "  content_hash you carry cannot match the one the peer computes over its" >&2
  echo "  re-encode, and the L1 put answers 400 hash_mismatch — silently, because" >&2
  echo "  dispatch_write is fire-and-forget." >&2
  echo "" >&2
  echo "  If your bytes genuinely do NOT become an Entity's data, lower the" >&2
  echo "  baseline in the same commit and say why in a comment at the call site." >&2
  echo "" >&2
  grep -rn "ciborium::into_writer(" src/ --include=*.rs | grep -vE "^[^:]+:[0-9]+: *//" >&2
  exit 1
fi

if [ "$ACTUAL" -lt "$EXPECTED" ]; then
  echo "ecf-lint: FAIL — $ACTUAL uses, baseline is $EXPECTED. The ratchet only goes DOWN" >&2
  echo "  with its baseline: lower $BASELINE_FILE to $ACTUAL in this same commit." >&2
  exit 1
fi

echo "ecf-lint: OK — $ACTUAL non-entity ciborium::into_writer use(s), matches the baseline"

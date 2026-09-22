#!/usr/bin/env bash
# foreign-cache-lint — the static half of D24: **no durable copy of someone
# else's bytes without a currency trigger.**
#
# **A discipline with no enforcement point does not count.** D24 has two, and
# this script is one of them; the other is behavioural and lives in
# `tests/e2e_worker.rs` as
# `an_app_republished_under_a_stable_identity_reaches_a_returning_profile` —
# publish, boot, republish under the SAME identity, boot again, assert the new
# bytes are on screen.
#
# The two halves are not redundant and neither subsumes the other:
#
#   * The GATE proves a returning profile sees a republish, and it can only
#     prove it for the artifact it exercises (an app bundle).
#   * This LINT proves no NEW consumer has appeared that fetches a foreign
#     artifact on its own terms. It cannot tell whether that consumer would be
#     correct — only that it did not go through the entry point that is.
#
# What it checks
# --------------
#
# Direct calls to the per-artifact fetchers in `content_site::http_poll` from
# anywhere but `content_site::foreign_cache`, which is the single entry point
# that owns presence **and currency** (`ensure_current`).
#
# Why the MODULE BOUNDARY and not the defect's own shape
# ------------------------------------------------------
#
# The audit that earned this rule proposed linting the shape of the bug: a
# durable read of a foreign-qualified path (`/{me}/{foreign}/…`) used as the
# condition guarding a fetch. That is the true rule, and it is a fragile grep —
# the condition can be a helper, a `let`-binding two lines up, a `match`, an
# `Option` combinator. **AP29 is the catalog entry for a gate that counts prose**,
# and a pattern that subtle would be exactly that.
#
# The boundary enforces the same rule from the other side, mechanically: a
# consumer that cannot reach the fetch API cannot express "only if absent",
# because `Currency` has no variant that means it. What this cannot catch is a
# consumer that calls `ensure_current` *conditionally* on holding a copy — and
# that is what the gate is for.
#
# Enumerations (`fetch_sites_list`, `fetch_pages_list`) are deliberately NOT
# counted: a `.list` is fetched fresh on every use and never becomes a durable
# copy, so it has no staleness to model. `resolve_closure_via` likewise — it is
# a pure-network resolve with no store read, which is precisely why a stale
# cached manifest never produced a stale page body.
#
# Baseline-ratcheted, like `tools/net-lint.sh` and `tools/ui-lint.sh`: the counts
# must EQUAL `tools/foreign-cache-lint-baseline.txt`. Went UP → route it through
# `foreign_cache::ensure_current`, or — if it genuinely is not a durable copy —
# say why in a comment and ratchet the baseline in the SAME commit:
#
#     ./tools/foreign-cache-lint.sh --update-baseline
#
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=tools/foreign-cache-lint-baseline.txt

# The fetchers that produce a DURABLY CACHED foreign artifact, plus the two raw
# hops. Every one of these has a `ForeignArtifact` variant behind it.
PATTERN='fetch_manifest(\|fetch_app_catalog(\|fetch_app_bundle(\|fetch_asset(\|fetch_pointer(\|fetch_content('

count() {
    { grep -o "$1" <<<"$2" || true; } | wc -l
}

# Whole-line `//` comments are dropped before counting, for the reason
# `net-lint.sh` learned the hard way: every call site this gate protects carries
# a comment naming the call it replaced, and a lint that charges you for
# documenting the rule teaches people to stop documenting it.
flatten() {
    sed '/^[[:space:]]*\/\//d' "$1" | tr '\n' ' '
}

report() {
    find src -name '*.rs' | LC_ALL=C sort | while read -r f; do
        # The entry point itself, and the module that DEFINES these functions.
        [[ "$f" == "src/content_site/foreign_cache.rs" ]] && continue
        [[ "$f" == "src/content_site/http_poll.rs" ]] && continue
        flat=$(flatten "$f")
        raw=$(count "$PATTERN" "$flat")
        if ((raw > 0)); then
            printf '%s direct_foreign_fetch=%d\n' "$f" "$raw"
        fi
    done
}

if [[ "${1:-}" == "--update-baseline" ]]; then
    report >"$BASELINE"
    echo "foreign-cache-lint: baseline updated →"
    cat "$BASELINE"
    exit 0
fi

if [[ ! -f "$BASELINE" ]]; then
    echo "foreign-cache-lint: no baseline at $BASELINE — create it with --update-baseline" >&2
    exit 1
fi

current=$(report)
if ! diff -u "$BASELINE" <(printf '%s\n' "$current") >/tmp/foreign-cache-lint.diff 2>&1; then
    echo "foreign-cache-lint: FAILED — direct foreign-artifact fetch counts moved (D24)." >&2
    echo >&2
    cat /tmp/foreign-cache-lint.diff >&2
    echo >&2
    echo "A durable copy of someone else's bytes needs a currency trigger, and" >&2
    echo "'I already have one' is not a trigger. Route the fetch through" >&2
    echo "content_site::foreign_cache::ensure_current, which always issues hop 1" >&2
    echo "(the 58-byte pointer, no-store) and downloads the body only if it moved." >&2
    echo >&2
    echo "If the new call genuinely produces no durable copy, ratchet the baseline" >&2
    echo "in the same commit: ./tools/foreign-cache-lint.sh --update-baseline" >&2
    exit 1
fi

echo "foreign-cache-lint: OK (counts match $BASELINE)"

#!/usr/bin/env bash
# net-lint — the static half of D23: no unbounded network await on the boot path.
#
# **A discipline with no enforcement point does not count.** D23 has three, and
# this script is two of them; the third is behavioural and lives in
# `tests/e2e_worker.rs` as `boot_survives_a_blackholed_deployment_config` (G1)
# plus `a_cached_shell_survives_a_blackholed_origin` (G1/SW).
#
# The two halves are not redundant and neither subsumes the other:
#
#   * The GATE proves a deadline is honoured, and it can only prove it for the
#     fetches that exist today on the paths it exercises.
#   * This LINT proves no NEW raw fetch has appeared. It cannot tell whether a
#     deadline is respected — only that the chokepoint was used.
#
# What it checks
# --------------
#
#   rust — `fetch_with_str` / `fetch_with_request` / `fetch_with_str_and_init`
#          anywhere but `src/net.rs` and the allowlist below. The chokepoint is
#          `net::fetch_text_bounded`, which bounds headers AND body under one
#          deadline.
#
#   sw   — bare `fetch(` calls in `assets/sw.js`. A call written as
#          `fetchWithDeadline(` does not match, so the count is exactly the
#          number of unbounded ones; which of them are deliberate is carried by
#          the baseline, per the boundary note below.
#
#   html — the same rule for `index.html`, and it was a BLIND SPOT: the first
#          version of this gate checked Rust and `sw.js` only, while `index.html`
#          carries the service-worker registration *and* the L1 System Recovery
#          BIOS. That is boot-path JS by any reading — and the BIOS is worse than
#          the boot path, because it is what you reach when boot has already
#          failed. An unbounded read there hangs the diagnostic you opened
#          precisely because something was already hanging. Covered from
#          2026-08-27, when the recovery console gained its first network probes.
#
# Both are baseline-ratcheted, like `tools/ui-lint.sh`: the counts must EQUAL
# `tools/net-lint-baseline.txt`. Went UP → you added a raw fetch; route it
# through the chokepoint, or — if it genuinely has no fallback to take (see
# below) — say so in a comment and ratchet the baseline in the SAME commit:
#
#     ./tools/net-lint.sh --update-baseline
#
# The boundary of the rule, because it is the part that gets misapplied
# -------------------------------------------------------------------
#
# **D23 bounds an await that is BLOCKING A DEFINED ALTERNATIVE OUTCOME. It is
# not a rule about the word `fetch`.** A deadline is only ever an improvement
# when there is something else to do on expiry:
#
#   * `deployment_config::fetch()` has one — build-time defaults (D16). Bound it.
#   * `networkFirst` has one — the cached shell, sitting one line below an await
#     that never returned. Bound it.
#   * `cacheFirst` on a hashed asset that is NOT cached has none: the URL is the
#     version, there is no older copy, and the only alternative to waiting is a
#     503. A deadline there converts a slow first download of the ~30 MB main
#     bundle into a hard failure on precisely the connections least able to
#     afford it. Leave it, and the baseline carries it honestly rather than the
#     whole file being exempted.
#
# The remaining allowlisted Rust call site (`content_site/http_poll.rs`) is not
# on the boot path — it runs behind a rendered surface, after the frame loop is
# already running, where a slow fetch is a slow panel and not a blank page.
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=tools/net-lint-baseline.txt

# Occurrences of $1 (basic regex) in $2. `grep` exits 1 on zero matches, which
# `set -e -o pipefail` would turn into a hard abort, so swallow that case.
count() {
    { grep -o "$1" <<<"$2" || true; } | wc -l
}

# Flatten a file to one line for counting, **with `//` comment lines dropped
# first**.
#
# Not a detail. Every call site this gate protects is one whose comment explains,
# by name, the raw call it is replacing or deliberately keeping — so the first
# version of this script counted those sentences and reported a violation in the
# very file that had just been fixed. A lint that charges you for documenting the
# rule teaches people to stop documenting it.
#
# Whole-line comments only — deliberately NOT a strip-from-`//`-to-end rule,
# which would also truncate any code line holding an `https://` literal and
# silently stop counting whatever followed it.
flatten() {
    sed '/^[[:space:]]*\/\//d' "$1" | tr '\n' ' '
}

report() {
    # --- Rust: raw web-sys fetch outside the chokepoint -------------------
    find src -name '*.rs' | LC_ALL=C sort | while read -r f; do
        [[ "$f" == "src/net.rs" ]] && continue
        flat=$(flatten "$f")
        raw=$(count 'fetch_with_\(str\|request\)' "$flat")
        if ((raw > 0)); then
            printf '%s rust_raw_fetch=%d\n' "$f" "$raw"
        fi
    done
    # --- Service worker: bare fetch outside the deadline helper -----------
    #
    # `[^a-zA-Z]fetch(` does not match `fetchWithDeadline(`, so a bounded call
    # is invisible here and only the raw ones are counted. The helper's own body
    # necessarily contains one — it is the single place a bare `fetch` is
    # supposed to appear — and the baseline carries it.
    flat=$(flatten assets/sw.js)
    sw_raw=$(count '[^a-zA-Z]fetch(' "$flat")
    if ((sw_raw > 0)); then
        printf 'assets/sw.js sw_raw_fetch=%d\n' "$sw_raw"
    fi
    # --- index.html: the SW registration and the L1 recovery BIOS ---------
    #
    # Same counting rule, same reason. The BIOS's own `fetchWithDeadline` helper
    # holds the one bare `fetch` that is supposed to exist here; anything above
    # that is a network read added to the console people open when the app is
    # already broken.
    flat=$(flatten index.html)
    html_raw=$(count '[^a-zA-Z]fetch(' "$flat")
    if ((html_raw > 0)); then
        printf 'index.html html_raw_fetch=%d\n' "$html_raw"
    fi
}

if [[ "${1:-}" == "--update-baseline" ]]; then
    report >"$BASELINE"
    echo "net-lint: baseline updated →"
    cat "$BASELINE"
    exit 0
fi

if [[ ! -f "$BASELINE" ]]; then
    echo "net-lint: no baseline at $BASELINE — create it with --update-baseline" >&2
    exit 1
fi

current=$(report)
if ! diff -u "$BASELINE" <(printf '%s\n' "$current") >/tmp/net-lint.diff 2>&1; then
    echo "net-lint: FAILED — the raw-fetch counts moved (D23)." >&2
    echo >&2
    cat /tmp/net-lint.diff >&2
    echo >&2
    echo "A count that went UP means a network read was added outside the" >&2
    echo "bounded chokepoint. Route it through net::fetch_text_bounded (Rust)" >&2
    echo "or fetchWithDeadline (sw.js). If it genuinely has no alternative" >&2
    echo "outcome to take on expiry — see the boundary note at the top of this" >&2
    echo "script — say so in a comment beside it and ratchet the baseline in" >&2
    echo "the same commit: ./tools/net-lint.sh --update-baseline" >&2
    exit 1
fi

echo "net-lint: OK (D23 — raw-fetch counts match the baseline)"

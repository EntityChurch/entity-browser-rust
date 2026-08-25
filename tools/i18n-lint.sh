#!/usr/bin/env bash
# i18n-lint — the ratchet gate for the i18n program (design §4, §3.3).
#
# The string+direction twin of ui-lint: it makes two RTL/translation hazards
# cost something so the migration converges and no regression creeps back. Same
# mechanism as ui-lint — a checked-in baseline the current count must EQUAL,
# ratcheted DOWN (never up). Two per-file metrics:
#
#   raw  — un-extracted user-facing text (route through `crate::i18n::t()`).
#          OPT-OUT prose detection (tools/i18n_prose_scan.py).
#   phys — physical-direction CSS props that break RTL (margin/padding/border-
#          {left,right}, text-align:{left,right}, float) — use the logical
#          equivalent (*-inline-start/-end, start/end). Swept to 0 in P3;
#          held there in ALL files (physical CSS is never "defining material").
#
# HISTORY (2026-07-19): `raw` WAS opt-in-anchored — it counted literals in only
# five call positions (card/field/button/button_action/set_text_content + a
# title-attr). That read raw=0 while ~440 user-facing strings sat un-extracted
# across the app (checkbox/radio labels, subheadings, table headers, td_text,
# loading/empty/error, hint text, format!-assembled sentences, the status bar) —
# every non-anchor DOM-emit path was invisible. See
# docs/plans/AUDIT-I18N-COVERAGE-GAP-2026-07-19.md. The metric is now OPT-OUT:
# a string literal is counted as translatable prose UNLESS excluded (CSS / URI /
# tree-path / i18n key / log-or-diagnostic / value-render / key-name / test) or
# explicitly suppressed with `// i18n-ignore` (one line) or `//! i18n-ignore-file`
# (a wholly dev-facing file). The detector (tools/i18n_prose_scan.py) documents
# the rules; it runs on python3 (present in the build image) and is deterministic.
#
# Counts went UP  → new un-extracted prose; route it through `crate::i18n::t(key)`
#                   (+ add the key to EN & the overlays), OR mark it dev-facing
#                   with `// i18n-ignore` if it must stay English (design §6).
# Counts went DOWN → you extracted a surface; ratchet in the SAME commit:
#                    ./tools/i18n-lint.sh --update-baseline
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=tools/i18n-lint-baseline.txt

# Occurrences of $1 (basic regex) in $2 — grep exits 1 on zero matches, which
# `set -e -o pipefail` would turn into a hard abort, so swallow that case.
count() {
    { grep -o "$1" <<<"$2" || true; } | wc -l
}

report() {
    # raw — un-extracted translatable UI prose. OPT-OUT (2026-07-19): the old
    # metric counted literals in only five anchor positions and so read 0 while
    # ~440 strings sat un-extracted (AUDIT-I18N-COVERAGE-GAP-2026-07-19). It is
    # now the opt-out prose scanner (tools/i18n_prose_scan.py — see its header:
    # counts prose UNLESS excluded/`// i18n-ignore`/`//! i18n-ignore-file`).
    # python3 is in the build image; the scan is deterministic.
    local raw_lines phys_lines
    raw_lines=$(python3 tools/i18n_prose_scan.py)

    # phys — physical-direction CSS props that have a logical equivalent
    # (DESIGN §3.3): margin/padding/border-{left,right}, text-align:{left,right},
    # float:{left,right}. RTL-hostile — use *-inline-start/end and start/end.
    # Counted in ALL render files (physical CSS is never a file's "defining raw
    # material"). Excludes bare `left:`/`right:` insets (ambiguous) — those are
    # caught by the pseudo-locale visual pass.
    phys_lines=$(
        find src/dom src/views src/app_host -name '*.rs' | LC_ALL=C sort | while read -r f; do
            flat=$(tr '\n' ' ' <"$f")
            phys=$(count 'margin-left' "$flat")
            phys=$((phys + $(count 'margin-right' "$flat")))
            phys=$((phys + $(count 'padding-left' "$flat")))
            phys=$((phys + $(count 'padding-right' "$flat")))
            phys=$((phys + $(count 'border-left:' "$flat")))
            phys=$((phys + $(count 'border-right:' "$flat")))
            phys=$((phys + $(count 'text-align: *left' "$flat")))
            phys=$((phys + $(count 'text-align: *right' "$flat")))
            phys=$((phys + $(count 'float: *left' "$flat")))
            phys=$((phys + $(count 'float: *right' "$flat")))
            if ((phys > 0)); then printf '%s phys=%d\n' "$f" "$phys"; fi
        done
    )

    # Merge by path → `path raw=R phys=P` for any file with R+P>0, C-sorted so
    # the baseline order is stable and locale-independent.
    { printf '%s\n' "$raw_lines"; printf '%s\n' "$phys_lines"; } | awk '
        $2 ~ /^raw=/  { split($2, a, "="); raw[$1]  = a[2] }
        $2 ~ /^phys=/ { split($2, a, "="); phys[$1] = a[2] }
        END {
            for (f in raw)  seen[f] = 1
            for (f in phys) seen[f] = 1
            for (f in seen) {
                r = raw[f] + 0; p = phys[f] + 0
                if (r + p > 0) printf "%s raw=%d phys=%d\n", f, r, p
            }
        }' | LC_ALL=C sort
}

if [[ "${1:-}" == "--update-baseline" ]]; then
    # Normalize exactly as the compare path does (`printf '%s\n' "$current"`),
    # so the all-clean state (report empty → one trailing newline) round-trips
    # instead of spuriously diverging (0-byte file vs a lone newline).
    printf '%s\n' "$(report)" >"$BASELINE"
    echo "i18n-lint: baseline updated ($BASELINE)"
    exit 0
fi

if [[ ! -f "$BASELINE" ]]; then
    echo "i18n-lint: FAIL — no baseline; generate it: ./tools/i18n-lint.sh --update-baseline" >&2
    exit 1
fi

current=$(report)
if ! diff_out=$(diff -u "$BASELINE" <(printf '%s\n' "$current")); then
    echo "$diff_out" >&2
    cat >&2 <<'EOF'
i18n-lint: FAIL — i18n counts diverged from the baseline (-baseline / +current).
  raw UP:   route the new label through crate::i18n::t("key") + add the key
            to the `en` catalog (src/i18n.rs EN).
  phys UP:  a new physical-direction CSS prop — use the logical equivalent
            (margin-inline-start/-end, padding-inline-*, border-inline-*,
            text-align:start/end).
  Counts DOWN: you migrated a surface — ratchet the baseline in this commit:
               ./tools/i18n-lint.sh --update-baseline
EOF
    exit 1
fi

total=$(awk '{r+=substr($2,5); p+=substr($3,6)} END {printf "raw=%d phys=%d", r, p}' "$BASELINE")
echo "i18n-lint: OK — matches baseline ($total across $(wc -l <"$BASELINE") files)"

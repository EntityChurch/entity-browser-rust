#!/usr/bin/env bash
# ui-lint — the ratchet gate for the UI component standard (S8).
#
# "A standard without a gate is a suggestion" — the 2026-07-14 UI audit's
# core finding (AUDIT-UI-STANDARDIZATION-2026-07-14 §5.2): the S1–S8 standard
# existed, nothing failed a diff that violated it, so every window drifted.
# This script makes drift cost something. It counts, per file under src/dom/
# and src/views/:
#
#   atoms  — raw create_element("button"|"input"|"select"|"textarea");
#            build these via dom/components.rs (button/text_input/select).
#   styles — inline style STRING literals: `"style", "…"` / `"style", &format!`;
#            use the theme.rs tokens (BTN_*, INPUT, SP_*) instead.
#   hex    — hex colors not wrapped as `var(--token, #literal)`;
#            reference colors through theme tokens (REFERENCE-THEMING).
#
# The checked-in baseline (tools/ui-lint-baseline.txt) is the ratchet: the
# current counts must EQUAL it. Counts went UP → you hand-rolled an atom; use
# the components:: primitives. Counts went DOWN → good, you migrated; ratchet
# the baseline in the SAME commit:  ./tools/ui-lint.sh --update-baseline
#
# Allowlisted (the files that DEFINE the primitives/styles, so raw material
# is their job): dom/components.rs, dom/theme.rs, dom/style.rs, dom/util.rs,
# dom/content_site.rs (the site overlay's stylesheet lives there). Documented
# intentionally-raw uses elsewhere (QR black/white, knowledge-base syntax
# colors — REFERENCE-THEMING §8) are carried honestly in the baseline instead
# of exempting their whole files.
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=tools/ui-lint-baseline.txt
ALLOW='^src/dom/(components|theme|style|util|content_site)\.rs$'

# Occurrences of $1 (basic regex) in $2 — grep exits 1 on zero matches, which
# `set -e -o pipefail` would turn into a hard abort, so swallow that case.
count() {
    { grep -o "$1" <<<"$2" || true; } | wc -l
}

report() {
    find src/dom src/views -name '*.rs' | LC_ALL=C sort | while read -r f; do
        [[ "$f" =~ $ALLOW ]] && continue
        # Flatten so multi-line calls (rustfmt splits args) still match.
        flat=$(tr '\n' ' ' <"$f")
        atoms=$(count 'create_element("\(button\|input\|select\|textarea\)"' "$flat")
        styles=$(count '"style",[[:space:]]*[&"]' "$flat")
        hex_all=$(count '#[0-9a-fA-F]\{3,8\}' "$flat")
        hex_ok=$(count 'var(--[a-zA-Z0-9_-]*,[[:space:]]*#[0-9a-fA-F]\{3,8\}' "$flat")
        hex=$((hex_all - hex_ok))
        if ((atoms + styles + hex > 0)); then
            printf '%s atoms=%d styles=%d hex=%d\n' "$f" "$atoms" "$styles" "$hex"
        fi
    done
}

if [[ "${1:-}" == "--update-baseline" ]]; then
    report >"$BASELINE"
    echo "ui-lint: baseline updated ($BASELINE)"
    exit 0
fi

if [[ ! -f "$BASELINE" ]]; then
    echo "ui-lint: FAIL — no baseline; generate it: ./tools/ui-lint.sh --update-baseline" >&2
    exit 1
fi

current=$(report)
if ! diff_out=$(diff -u "$BASELINE" <(printf '%s\n' "$current")); then
    echo "$diff_out" >&2
    cat >&2 <<'EOF'
ui-lint: FAIL — raw-UI counts diverged from the baseline (above: -baseline / +current).
  Counts UP:   don't hand-roll atoms/styles — use dom/components.rs (button,
               text_input, select, field) and dom/theme.rs tokens.
  Counts DOWN: you migrated a surface — ratchet the baseline in this commit:
               ./tools/ui-lint.sh --update-baseline
EOF
    exit 1
fi

total=$(awk '{a+=substr($2,7); s+=substr($3,8); h+=substr($4,5)} END {printf "atoms=%d styles=%d hex=%d", a, s, h}' "$BASELINE")
echo "ui-lint: OK — matches baseline ($total across $(wc -l <"$BASELINE") files)"

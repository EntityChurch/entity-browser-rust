#!/usr/bin/env bash
# i18n-lint — the ratchet gate for the i18n program (design §4, §3.3).
#
# The string+direction twin of ui-lint: it makes two RTL/translation hazards
# cost something so the migration converges and no regression creeps back. Same
# mechanism as ui-lint — a checked-in baseline the current count must EQUAL,
# ratcheted DOWN (never up). Two per-file metrics:
#
#   raw  — un-extracted user-facing text (route through `crate::i18n::t()`);
#          the P4 long-tail. Opt-in-anchored (see below).
#   phys — physical-direction CSS props that break RTL (margin/padding/border-
#          {left,right}, text-align:{left,right}, float) — use the logical
#          equivalent (*-inline-start/-end, start/end). Swept to 0 in P3;
#          held there in ALL files (physical CSS is never "defining material").
#
# CRITICAL: this is **opt-in-anchored, not opt-out** (design §4, review
# finding 3). Of the ~329 interpolated `format!`s, most are NOT translatable
# prose — URIs (`entity://{peer}/...`), CSS (`--site-bg:{bg}`), inspector value
# renderings (`Float({})`), `tracing` diagnostics, logged `Err(...)`. A naive
# "flag every literal" gate would be pure noise and get disabled. So we count
# literals ONLY in known UI-emitting call positions — the label/title args of
# the component atoms and direct DOM text — where a literal genuinely SHOULD be
# a `t()` key. A `t("...")` call is not a bare literal in these positions, so
# migrating a site drops its count.
#
# Anchors counted per file under src/dom/ + src/views/ (newlines flattened so
# rustfmt-split calls still match):
#
#   card("…")                    — window/card title
#   field("…", …)                — settings/field label
#   button(ctx, "…", …)          — button label
#   button_action(ctx, "…", …)   — button label
#   set_text_content(Some("…"))  — direct DOM text
#
# The anchor set is intentionally conservative (high-signal, low false-positive)
# and GROWS over time — like ui-lint's rules did. Adding an anchor is a
# deliberate ratchet step, not a silent tightening.
#
# Counts went UP  → a new raw label; route it through `crate::i18n::t(key)`.
# Counts went DOWN → you migrated a surface; ratchet in the SAME commit:
#                    ./tools/i18n-lint.sh --update-baseline
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=tools/i18n-lint-baseline.txt
# Files that DEFINE the atoms (their param plumbing mentions the names) or the
# i18n machinery itself — raw material is their job, exactly like ui-lint.
ALLOW='^src/dom/(components|theme|style|util)\.rs$'

# Occurrences of $1 (basic regex) in $2 — grep exits 1 on zero matches, which
# `set -e -o pipefail` would turn into a hard abort, so swallow that case.
count() {
    { grep -o "$1" <<<"$2" || true; } | wc -l
}

report() {
    find src/dom src/views -name '*.rs' | LC_ALL=C sort | while read -r f; do
        flat=$(tr '\n' ' ' <"$f")

        # phys — physical-direction CSS props that have a logical equivalent
        # (DESIGN §3.3): margin/padding/border-{left,right}, text-align:
        # {left,right}, float:{left,right}. RTL-hostile — use *-inline-start/end
        # and start/end. Counted in ALL files (physical CSS is never a file's
        # "defining raw material", unlike the atom constructors) so the shared
        # atom layer (style/theme/components) is held to 0 too. Excludes bare
        # `left:`/`right:` insets (ambiguous: symmetric `left:8px;right:8px` is
        # dir-neutral) — those are caught by the pseudo-locale visual pass.
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

        # raw — un-extracted UI-text literals (skip the atom-defining files).
        raw=0
        if [[ ! "$f" =~ $ALLOW ]]; then
            # Trailing `[^"]` requires a NON-EMPTY literal: an empty title
            # (`card("")`, site_editor's disclosure-form convention) is not
            # translatable prose, so it must not count.
            card=$(count 'card("[^"]' "$flat")
            # `[^.]field("` — the free-function UI atom `field("Label", …)`, NOT
            # the `.field("name", &val)` method of `fmt::DebugStruct` in a Debug
            # impl (views/*/model.rs). Those are struct field *names*, never
            # user-rendered — counting them was a false positive (they must not
            # be translated). Requiring a non-`.` lead char excludes the method.
            field=$(count '[^.]field("[^"]' "$flat")
            button=$(count 'button(ctx,[[:space:]]*"[^"]' "$flat")
            button=$((button + $(count 'button_action(ctx,[[:space:]]*"[^"]' "$flat")))
            text=$(count 'set_text_content(Some("[^"]' "$flat")
            # `"title", "…"` — a title-attribute tooltip literal (set via
            # util::set_attr / .set_attribute). A `t()` call has no quote after
            # the comma, so migrating drops the count. The `[^"]` keeps it from
            # matching an empty title. (text_input's field_id arg is `"title",
            # &initial` — a non-quote follows, so it's not caught here.)
            title=$(count '"title", *"[^"]' "$flat")
            raw=$((card + field + button + text + title))
        fi

        if ((raw + phys > 0)); then
            printf '%s raw=%d phys=%d\n' "$f" "$raw" "$phys"
        fi
    done
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

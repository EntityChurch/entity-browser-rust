#!/usr/bin/env python3
"""C15 — hold the DOCUMENTED cache rule to the executable one.

`docs/PUBLISHING-QUICKSTART.md` §6.2 is the fourth expression of the
cache-immutability rule and the only one that reaches a real CDN: it is the
worked Cloudflare recipe an operator copies. It is prose, so it cannot be run
against `tools/cache-policy-vectors.txt` like the other three.

What it CAN be held to is the thing that actually went wrong. The four
expressions did not drift because someone rewrote the policy; they drifted
because each was a hand-written restatement, and a hand-written restatement of a
regex is a regex nobody runs. So this asserts the document contains the *same
match expressions* the code uses, character for character, and that §6.1 does
not tell operators to copy something weaker.

This is deliberately a spelling check, not a semantic one — it cannot prove the
CDN recipe is correct, only that it has not silently drifted from the rule the
rest of the repo enforces. That is the honest limit of gating prose, and it is
strictly more than the comment it replaces.
"""
import re
import sys
from pathlib import Path

DOC = Path("docs/PUBLISHING-QUICKSTART.md")

# The two match expressions the CDN recipe must carry. These are the documented
# spellings of `src/cache_policy_rule.rs` / `tools/cors-serve.py`; if you change
# the rule, change them here and in §6.2 in the same commit.
REQUIRED = {
    "the content-store shard regex": r"content/[0-9a-f]{2}/[0-9a-f]{2}/[0-9a-f]{32,}",
    "the hash-named bundle regex": r"-[0-9a-f]{8,}(_bg)?\.(wasm|js)$",
}

# Retired spellings that must not appear in the RULES TABLE.
#
# **Scoped to table rows (lines starting with `|`) on purpose, and this is the
# interesting part of the check.** A plain document-wide grep flagged the
# document's own note explaining which spelling was retired and why — and a doc
# that may not name the rule it replaced cannot warn anyone off it. The
# copy-paste surface is the table; prose about the table is not a hazard. This is
# the same distinction the repo's other baseline-ratcheted linters draw between a
# call site and a comment about one.
# Each carries its OWN consequence: the two retired spellings fail in OPPOSITE
# directions, and a shared message would name the wrong one half the time — the
# same collapsed-outcome mistake the rest of this repo keeps catching.
FORBIDDEN = {
    'starts with "/content/"': (
        "the PREFIX test",
        "It is safe but it silently drops every PREFIXED deployment out of immutable "
        "caching — `/docs/content/…`, `/protocol/content/…`. `dist-federation` emits 92 "
        "such blobs and nothing would tell the operator.",
    ),
    'contains "content/"': (
        "the SUBSTRING test",
        "It classifies an ingested Hugo/Zola/Lektor site's own `content/` tree as "
        "immutable — mutable HTML at a stable URL, pinned for a year. Brick-matrix cell "
        "#9 has no remedy at all.",
    ),
}


def main() -> int:
    if not DOC.exists():
        print(f"cache-policy-doc-check: {DOC} is missing", file=sys.stderr)
        return 1
    raw = DOC.read_text(encoding="utf-8")
    # Markdown escapes `|` inside a table cell. The document is carrying the same
    # regex, spelled for a table; normalise before comparing, or this gate fails
    # on the escaping rather than on the rule.
    text = raw.replace(r"\|", "|")
    table_rows = "\n".join(l for l in text.splitlines() if l.lstrip().startswith("|"))

    problems = []
    for label, needle in REQUIRED.items():
        if needle not in text:
            problems.append(
                f"  MISSING {label}:\n    {needle}\n"
                f"    §6.2 is the recipe an operator copies into a CDN. If the rule moved "
                f"and this document did not, every deployment that follows the docs gets "
                f"the OLD rule — which is how the four expressions diverged."
            )
    for needle, (label, why) in FORBIDDEN.items():
        if needle in table_rows:
            problems.append(
                f"  PRESENT (must not be) — {label}:\n    {needle}\n    {why}"
            )

    if problems:
        print("cache-policy-doc-check: FAIL — PUBLISHING-QUICKSTART has drifted from the rule")
        print("\n".join(problems))
        return 1

    print(
        f"cache-policy-doc-check: OK — {DOC.name} carries both match expressions "
        f"and none of the retired spellings"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

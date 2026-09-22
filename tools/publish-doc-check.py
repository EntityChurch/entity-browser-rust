#!/usr/bin/env python3
"""Hold `REFERENCE-PUBLISHING-PIPELINE.md` to the code it describes.

Earned by `docs/plans/AUDIT-THE-PUBLISHING-PIPELINE-AND-WHY-ITS-MODEL-IS-UNFINDABLE-2026-09-10.md`,
which measured that document 17 days stale and wrong in the expensive direction:
**two SHIPPED capabilities were recorded as deferred** (a shipped capability
recorded as deferred reads as work still owed), and the flag reference listed
**10 of 22** flags — the missing twelve including `--verify`, `--set-home` and
`--supersede`, all three load-bearing.

## What this can and cannot do — read this before trusting a green run

It gates the **enumerable** half only: the flag set, and the file count it
claims. **It cannot check prose**, so it cannot tell you the model is still
stated correctly, that a "deferred" row is still deferred, or that §4's `dist/`
layout is current. Those are exactly the claims that went wrong, and only two of
them are mechanically checkable.

This is the same honest limit `cache-policy-doc-check.py` states for itself, and
it is stated here rather than left for a reader to infer from a green line —
`AGENTS.md`'s standing complaint is that *recording a gap is what makes it look
handled*, so the gap is printed on every successful run, not buried in a
docstring.

## Why the flag set is the right thing to gate

It is the part of the document an operator copies, it is enumerable from source
with no judgement, and it is where the drift actually was. A flag added to
`publish.rs` without a row here now fails `make lint` in the same commit.
"""
import re
import sys
from pathlib import Path

DOC = Path("docs/architecture/specs/REFERENCE-PUBLISHING-PIPELINE.md")
SRC = Path("src/content_site/publish.rs")
CONTENT_SITE = Path("src/content_site")

# The flag table lives between these two anchors. Scoped rather than
# document-wide because flags are named in prose elsewhere in the file (§5, §7),
# and a document-wide grep would accept a flag mentioned in passing as
# documented — which is the `cache-policy-doc-check` scoping lesson: a check
# that is too broad passes for the wrong reason.
TABLE_START = "### The `publish` verb"
TABLE_END = "## 4."

# `--strict-links` is accepted and ignored (it asks for today's default; other
# repos' pipelines still pass it). It must stay documented AS ignored, so it is
# expected in both sets and needs no special case.

FLAG_RE = re.compile(r'(--[a-z][a-z-]*)')


def fail(msg: str) -> None:
    print(f"publish-doc-check: FAIL — {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    for p in (DOC, SRC):
        if not p.exists():
            fail(f"{p} is missing")

    doc = DOC.read_text(encoding="utf-8")
    if TABLE_START not in doc or TABLE_END not in doc:
        fail(
            f"could not locate the flag table in {DOC} "
            f"(anchors {TABLE_START!r} … {TABLE_END!r}). If the section was "
            f"renamed, update the anchors here in the same commit."
        )
    section = doc.split(TABLE_START, 1)[1].split(TABLE_END, 1)[0]

    # Documented: flags inside backticks **in a table ROW**, not anywhere in the
    # section.
    #
    # Scoped this way after the first run of this very check failed its own
    # positive control on `--bind`, which appears in the explanatory blockquote
    # below the table (`"same spelling as `registry --bind`"`). That is
    # `cache-policy-doc-check`'s lesson arriving unprompted: a document-wide
    # grep accepts the document's own commentary as content, and here it did it
    # in the *strict* direction, reporting a documented flag the verb does not
    # accept. Rows only.
    # …and within a row, only backticked spans that **begin** with `--`.
    #
    # This rule took three attempts and each failure was instructive, so the
    # reasoning is here rather than the result alone:
    #
    #   1. Whole-section scan → reported `--bind` as documented. It appears in
    #      the explanatory blockquote under the table.
    #   2. Whole-row scan → still `--bind`: `--registry-pin`'s *description*
    #      cell reads "same spelling as `registry --bind`".
    #   3. First-column-only → broke on `` `--surface=<chrome|site|window>` ``,
    #      whose literal `|` splits the row, AND wrongly rejected `--ingest-games`
    #      and `--strict-links`, which are correctly documented **inside a
    #      description cell** — an alias and an accepted-and-ignored flag do not
    #      each need their own row.
    #
    # The structural discriminator is that a **flag reference** is a backticked
    # span starting with `--`, while a **command mention** (`registry --bind`)
    # starts with the verb. That needs no cell splitting, so the `|`-in-backticks
    # problem disappears, and it accepts a flag documented anywhere in the row.
    # No hand-maintained exclusion list — the thing that would rot.
    documented = set()
    for ln in section.splitlines():
        if not ln.lstrip().startswith("|"):
            continue
        for span in re.findall(r"`([^`]*)`", ln):
            if span.startswith("--"):
                documented.update(FLAG_RE.findall(span))
    # Real: the string literals `publish.rs` actually tests argv against.
    real = set(FLAG_RE.findall(" ".join(re.findall(r'"(--[a-z][a-z-]*)', SRC.read_text(encoding="utf-8")))))

    if not real:
        fail(f"found no flag literals in {SRC} — the extraction broke, not the doc")
    # Anti-vacuity: a table that lost its backticks would yield an empty set and
    # every "undocumented" flag would be reported, which is loud. The dangerous
    # direction is the opposite one, so guard it explicitly.
    if not documented:
        fail(f"the flag table in {DOC} yielded no flags — the table or its anchors moved")

    missing = sorted(real - documented)
    extra = sorted(documented - real)
    if missing:
        fail(
            f"{len(missing)} flag(s) in {SRC} are not in the flag table: "
            + ", ".join(missing)
            + " — add a row in this commit"
        )
    if extra:
        fail(
            f"{len(extra)} flag(s) documented but not accepted by {SRC}: "
            + ", ".join(extra)
            + " — the verb dropped them, or the row is aspirational"
        )

    # The file count the document claims about itself.
    actual_files = len(list(CONTENT_SITE.glob("*.rs")))
    m = re.search(r"`src/content_site/`\s*\(\*\*(\d+) files\*\*", doc)
    if not m:
        fail(
            "§8's `src/content_site/` file-count claim is not in the expected "
            "form '(**N files** as of YYYY-MM-DD)' — it drifted or was reworded"
        )
    claimed = int(m.group(1))
    if claimed != actual_files:
        fail(
            f"§8 claims src/content_site/ has {claimed} files; it has {actual_files}"
        )

    print(
        f"publish-doc-check: OK — flag table matches publish.rs exactly "
        f"({len(real)} flags), src/content_site/ file count {actual_files} agrees. "
        f"NOTE: prose is NOT checked — the model statement, the 'deferred' rows and "
        f"the §4 dist/ layout can still go stale without failing this gate."
    )


if __name__ == "__main__":
    main()

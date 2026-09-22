#!/usr/bin/env python3
"""fidelity-lint — a durable write of a DECODED view re-encodes, and the bytes move.

Why this is a lint and not a comment
------------------------------------

The rule — *an entity you did not author travels as bytes; a struct is only a
view of it* — was learned twice in the same function and generalised nowhere
in between:

  2026-09-10  `persist_to_cache` stores `asset.to_entity()`, so an asset arm
              this build could not read was written back hollow. Mitigated by
              SKIPPING the unreadable case, which left the re-encode in place.
  2026-09-16  The same three writes were measured: a manifest carrying a field
              we do not model came in at 52 bytes and went into the store at
              30, and — the arm that matters — an ordinary one-page site with
              no `nav` came in at 25 and went in at 30, because our encoder
              adds `nav: []`. No unknown field required.

The cost was not the fidelity. The write lands at the publisher's own path,
which is also `ForeignArtifact::Manifest::store_path`, and D24's currency check
compares the stored `content_hash` against the origin's 58-byte hop-1 pointer.
A rewritten copy's hash can never equal the pointer it came from, so
`ensure_current` answered `Fetched` on every sweep, forever.

Meanwhile `foreign_cache` (written *for* D24) carried the `Entity` and was
right, and `warm_site_writes` — whose own doc comment says it does *"the same
manifest-pinned write-through the browse resolver does"* — carried it and was
right too. Two writers at one key, one faithful and one not, and nothing
anywhere compared them. That is AP44: a rule needing the word *every*, written
as "and also do X here".

What it checks
--------------

Every durable write in product code whose value is `<name>.to_entity()` where
`<name>` was bound from a `from_entity` decode — i.e. a re-encode of something
we decoded rather than authored. Each such site must be named in the baseline
with a one-word reason.

**The baseline carries NAMES, not a count, and it fails BOTH ways.** This repo
already records what a count leaves open (`net-lint`, `ecf-lint`: a SWAP
passes) and the case that settled it (`vocab-lint`, where a finding changed
CLASS and a count-based baseline would have sailed through). Here a swap is
precisely the dangerous case: retire a deliberate re-encode while adding an
accidental one and the total is unchanged. So a baseline row with no live
finding is **also** a failure — a row outliving its debt silently re-admits it.

Deliberate is a real answer
---------------------------

Re-encoding is correct when we ARE the author: our own session config, a site
we are editing, a value we just constructed. Those are `deliberate` rows and
the reason field says which. What the baseline exists to stop is a NEW site
appearing with nobody having asked the question.

Stated limits, because a gate that overclaims is worse than none:

  * It is a **heuristic over one file at a time.** A decode in module A whose
    value is written in module B is invisible to it, as is a decode threaded
    through a struct field (which is exactly the shape `Obtained<T>` exists to
    make safe, and exactly what this cannot see).
  * It reads product code only — everything from the first `#[cfg(test)]`
    onward is skipped.
  * It flags on **name binding**, so a reused local name produces a false
    positive. Those are `not-a-decode` rows.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASELINE = ROOT / "tools" / "fidelity-lint-baseline.txt"

WRITES = ("seed_write", "dispatch_write", "seed_content", "put_and_wait", "content_put", ".put(")


def product_only(text: str) -> str:
    """Everything before the first test module. Product code is the subject."""
    i = text.find("#[cfg(test)]")
    return text[:i] if i >= 0 else text


def paren_args(text: str, open_at: int) -> str:
    """The balanced argument text of the call whose '(' follows `open_at`."""
    i = text.find("(", open_at)
    if i < 0:
        return ""
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "(":
            depth += 1
        elif text[j] == ")":
            depth -= 1
            if depth == 0:
                return text[i + 1 : j]
    return text[i + 1 :]


def findings() -> set[str]:
    """Two levels of taint, and the second is the one that matters.

    ⚠ **The first cut of this gate had ONE level and was falsified by the very
    defect it was written for.** The content-site manifest write read:

        let manifest_entity = rp.manifest.to_entity();   // the re-encode
        ...
        peers.seed_write(me, manifest_path(..), manifest_entity.clone());

    The write's argument is a *variable*, so a matcher looking for
    `<name>.to_entity()` inside the write's parentheses sees nothing. Reinstating
    the real bug left the gate green. The asset and page writes at the same site
    inline the call and WERE caught, which is what made the hole look like
    coverage.

    So: `decoded` are names bound from a `from_entity`, `reencoded` are names
    bound from `<decoded>.to_entity()`, and a write is a finding if its arguments
    re-encode a decoded value inline **or** mention a re-encoded binding.
    """
    found: set[str] = set()
    for path in sorted((ROOT / "src").rglob("*.rs")):
        src = product_only(path.read_text())
        decoded = {
            m.group(1)
            for m in re.finditer(
                r"let\s+(?:mut\s+)?(\w+)\s*(?::[^=]+)?=\s*[^;]*?from_entity", src, re.S
            )
        }
        if not decoded:
            continue
        # Level 2: `let x = <decoded>.to_entity();` — the binding the original
        # defect used, and the one a single-level matcher is blind to.
        reencoded: dict[str, str] = {}
        for m in re.finditer(
            r"let\s+(?:mut\s+)?(\w+)\s*(?::[^=]+)?=\s*([\w.]+)\s*\.\s*to_entity\s*\(\)", src
        ):
            if m.group(2).split(".")[0] in decoded:
                reencoded[m.group(1)] = m.group(2).split(".")[0]

        rel = path.relative_to(ROOT).as_posix()
        for verb in WRITES:
            for m in re.finditer(re.escape(verb), src):
                args = paren_args(src, m.start() + len(verb) - (1 if verb.startswith(".") else 0))
                if not args:
                    continue
                for call in re.finditer(r"([\w.]+)\s*\.\s*to_entity\s*\(\)", args):
                    base = call.group(1).split(".")[0]
                    if base in decoded:
                        found.add(f"{rel} {base}")
                for name, origin in reencoded.items():
                    if re.search(rf"\b{re.escape(name)}\b", args):
                        found.add(f"{rel} {origin}")
    return found


def read_baseline() -> dict[str, str]:
    rows: dict[str, str] = {}
    if not BASELINE.exists():
        return rows
    for line in BASELINE.read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) < 3:
            print(f"fidelity-lint: FAIL — malformed baseline row: {line!r}")
            print("  expected: <path> <binding> <reason>")
            sys.exit(1)
        rows[f"{parts[0]} {parts[1]}"] = parts[2]
    return rows


def main() -> int:
    live = findings()
    base = read_baseline()

    # Anti-vacuity. A rewritten matcher that finds nothing would pass silently
    # against an empty baseline, which is the failure mode this file is about.
    if not live and base:
        print("fidelity-lint: FAIL — found NOTHING while the baseline names rows.")
        print("  Either every row was genuinely retired (then empty the baseline in the")
        print("  same commit) or the matcher stopped matching. Do not assume the first.")
        return 1

    new = sorted(live - set(base))
    stale = sorted(set(base) - live)

    if new:
        print("fidelity-lint: FAIL — a durable write re-encodes a DECODED view.")
        print()
        print("  Re-encoding drops every field this build does not model, which moves")
        print("  the entity's address. If the value came from somebody else, that is a")
        print("  defect: hold it in `Obtained<T>` (src/obtained.rs) and write")
        print("  `.entity()`, which answers with the author's own bytes.")
        print()
        print("  If the re-encode is DELIBERATE — we authored the value, or the write IS")
        print("  an edit — add a baseline row saying so:")
        print()
        for row in new:
            print(f"      {row} deliberate-<why>")
        print()
        return 1

    if stale:
        print("fidelity-lint: FAIL — baseline rows with no live finding:")
        for row in stale:
            print(f"      {row}")
        print()
        print("  The set must match exactly, in both directions. A row that outlives its")
        print("  debt silently re-admits it, and a SWAP (one retired, one added) is the")
        print("  case a count-based baseline cannot see — which is why this one names.")
        return 1

    print(
        f"fidelity-lint: OK — {len(live)} decoded-view write(s), all named in the baseline. "
        "NOTE: single-file heuristic — a decode in one module written in another, or "
        "threaded through a struct field, is NOT visible to this gate."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

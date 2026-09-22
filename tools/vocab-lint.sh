#!/usr/bin/env bash
# vocab-lint — does OUR app-tier type vocabulary still line up with the corpus
# and the other seat? Baseline-ratcheted; holds today's debt, fails on new debt.
#
# WHY THIS EXISTS, AND WHY NO OTHER GATE COULD.
#
# Every other linter here reads our own tree. This one cannot, because the
# failure it looks for is not visible from inside one repo: two application
# seats can be perfectly wire-conformant and completely vocabulary-divergent
# **with no error anywhere**. `APP-CONVENTION-SHARE` §2 states the shape — a
# type-filtered query on the wrong tag returns *a correct, complete, EMPTY
# answer*. Nothing 404s. Nothing mismatches. The two sides simply never hold
# each other's data, and every test on both sides stays green.
#
# `spec vocab` (entity-system-arch-tools) is the analyzer for it. This wrapper
# runs it and holds US to what it finds. Arch's `AT-20` named the absence of
# exactly this instrument as the reason the application tier had been
# integrated by hand.
#
# SCOPED TO OUR OWN FINDINGS, DELIBERATELY.
#
# `spec vocab` reports the whole tier: tags the corpus declares that nobody
# implements, tags only the other seat emits, families that diverge. Most of
# those we cannot fix — `app/feed/*` being declared-unimplemented is a roadmap
# fact, not our debt, and `app/share/record` being single-seat is a statement
# about work we have not done yet rather than a defect in what we shipped.
#
# **A gate that fails on another party's board is a gate people learn to skip.**
# So this counts only findings that NAME `entity-browser-rust`: a tag we emit
# that no spec declares, a declared tag only we emit, and a family we are on the
# divergent side of. Everything else is printed for information and does not
# gate. The full board is `spec vocab` and belongs to arch.
#
# THE BASELINE CARRIES NAMES, NOT A COUNT.
#
# `net-lint` and `ecf-lint` ratchet on counts, and this repo's own guidance
# records the hole that leaves: **a SWAP passes.** Drop one item, add another,
# the count is unchanged and the gate stays green. Here a swap is precisely the
# dangerous case — retire `app/site-asset`'s declaration debt while minting a
# new undeclared tag and the tier is no better off. So the baseline is a SET of
# tags and the comparison is a set difference.
#
# IT SKIPS WHEN IT CANNOT LOOK, AND SAYS SO LOUDLY.
#
# It needs three sibling checkouts. A fresh clone that has only this repo must
# not fail its own lint — but a silent skip is the failure mode this repo has
# been bitten by more than once, so the skip names every missing piece and what
# went unchecked. `spec vocab` itself keeps the distinction (exit 2 =
# could-not-look) and so does this.
#
# ⛔ AND IT RESOLVES OUR SEAT BY DIRECTORY NAME, WHICH IS NOT THIS CHECKOUT WHEN
# YOU ARE IN A WORKTREE.
#
# `spec vocab` finds the seat at `<corpus parent>/entity-browser-rust` and reads
# whatever is on disk there. Run this from a worktree — `entity-browser-rust-vm`,
# say — and the gate measures the OTHER tree, at the other seat's head, **mid
# edit**, and attributes what it finds to your commit. Measured 2026-09-15: this
# worktree's lint went red on `single-seat app/feed/collection`, a tag that
# appears nowhere in this checkout outside `docs/status/` prose, because the main
# worktree had just landed the feed composer. It survived a full stash of the
# session's changes, which is the tell — *if the symptom survives your change
# being gone, the symptom is not yours.*
#
# Same class as the `Cargo.lock` hazard this repo already records: a tool that
# resolves a sibling BY NAME resolves it against whatever is on disk. The
# analyzer's own JSON carries `head` and `dirty` per seat and this wrapper used
# to throw both away, so nothing anywhere said which tree had been read. It now
# prints them on every run and refuses to speak for a tree that is not this one.
set -uo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
PARENT="$(dirname "$HERE")"
ARCH="$PARENT/entity-system-architecture"
TOOLS="$PARENT/entity-system-arch-tools"
BASELINE="$HERE/tools/vocab-lint-baseline.txt"
SEAT="entity-browser-rust"

skip() {
	echo "vocab-lint: SKIPPED — $1"
	echo "            NOT CHECKED: whether any tag we emit is undeclared, whether we"
	echo "            are alone on a declared tag, or whether a convention family has"
	echo "            diverged between the seats. This is the one class no test in"
	echo "            this repo can see on its own."
	exit 0
}

command -v python3 >/dev/null 2>&1 || skip "no python3"
[ -d "$ARCH/specs/applications" ] || skip "no $ARCH — the corpus that declares the tags"
[ -f "$TOOLS/spec-tool/cli.py" ] || skip "no $TOOLS — the analyzer"

# WHOSE TREE IS THE ANALYZER ABOUT TO READ? Checked BEFORE running it, because a
# report about another checkout is not a weaker answer, it is an answer to a
# different question — and one that names us in its findings.
SEAT_DIR="$PARENT/$SEAT"
seat_real="$(cd "$SEAT_DIR" 2>/dev/null && pwd -P)"
here_real="$(cd "$HERE" && pwd -P)"
if [ "$seat_real" != "$here_real" ]; then
	echo "vocab-lint: SKIPPED — the analyzer reads the seat at a fixed path and that is not this checkout."
	echo "            it would read: ${seat_real:-$SEAT_DIR}"
	if [ -n "$seat_real" ]; then
		echo "                        ($(git -C "$seat_real" log --oneline -1 2>/dev/null || echo 'not a checkout')$(
			[ -n "$(git -C "$seat_real" status --short 2>/dev/null)" ] && echo ', DIRTY')"
		echo "                         — another seat's work, attributed to your commit)"
	fi
	echo "            you are in:  $here_real"
	echo "            Run 'make lint' from $SEAT_DIR to exercise this gate. Rows found"
	echo "            there are that tree's to answer for, not this one's."
	skip "our seat resolves to a different tree"
fi

VOCAB_RAW="$(cd "$ARCH" && python3 "$TOOLS/spec-tool/cli.py" vocab --json 2>/dev/null)"
rc=$?
export VOCAB_RAW SEAT

# ONE python pass. It reads the JSON from the environment rather than stdin,
# because the reducer's own source arrives on stdin — a heredoc and a pipe
# cannot both have it, and when they collide `json.load(sys.stdin)` reads the
# empty remainder and the whole gate degrades to a skip that looks legitimate.
REDUCED="$(python3 <<'PY'
import json, os, sys
raw = os.environ.get("VOCAB_RAW", "")
seat = os.environ["SEAT"]
try:
    d = json.loads(raw)
except Exception:
    print("ERR\tnot-json"); sys.exit(0)

# `seats` is a LIST of seat records, not a count. Reading it as a number is how
# the first cut of this script reported "fewer than 2 seats" on a healthy run.
seats = d.get("seats") or []
n_seats = len(seats) if isinstance(seats, list) else int(seats or 0)
if n_seats < 2:
    print("ERR\tfewer-than-two-seats"); sys.exit(0)

f = d.get("findings", {})
rows = set()
for it in f.get("implemented-undeclared", []):
    if seat in (it.get("seats") or []):
        rows.add(("implemented-undeclared", it["tag"]))
for it in f.get("single-seat", []):
    if it.get("seat") == seat:
        rows.add(("single-seat", it["tag"]))
for it in f.get("divergent-family", []):
    if seat in (it.get("per_seat") or {}):
        rows.add(("divergent-family", "app/" + it["family"] + "/*"))

missing = d.get("missing_seats") or []
n_missing = len(missing) if isinstance(missing, list) else int(missing or 0)
# WHICH TREE, AT WHICH HEAD. The analyzer reports this per seat and the wrapper
# used to discard it, so a run could not be reproduced and a report about
# somebody else's checkout was indistinguishable from one about ours.
mine = next((s for s in seats if s.get("name") == seat), {})
print("SEAT\t%s%s" % (
    mine.get("head", "?"), ", DIRTY — this run is not reproducible" if mine.get("dirty") else ""))
print("CTX\t%d seat(s), %d missing, %s declared / %s emitted" % (
    n_seats, n_missing, d.get("declared_total"), d.get("implemented_total")))
for c, t in sorted(rows):
    print("ROW\t%s\t%s" % (c, t))
PY
)"

case "$REDUCED" in
	*"ERR"$'\t'"not-json"*) skip "spec vocab did not return JSON (exit $rc)" ;;
	*"ERR"$'\t'"fewer-than-two-seats"*)
		skip "spec vocab found fewer than 2 application seats — is $PARENT/entity-workbench-go present?" ;;
esac

CONTEXT="$(printf '%s\n' "$REDUCED" | sed -n 's/^CTX\t//p')"
SEAT_AT="$(printf '%s\n' "$REDUCED" | sed -n 's/^SEAT\t//p')"
OURS="$(printf '%s\n' "$REDUCED" | sed -n 's/^ROW\t//p')"
# `$here_real` is honest ONLY because the guard above proved the analyzer's seat
# path resolves to this checkout. Relax that guard and this line starts naming a
# directory the report is not about — which is the defect it was added for.
echo "vocab-lint: read $SEAT at $here_real ($SEAT_AT)"

[ -f "$BASELINE" ] || { echo "vocab-lint: FAIL — no baseline at $BASELINE"; exit 1; }
BASE="$(grep -vE '^\s*(#|$)' "$BASELINE" | sort)"
NOW="$(printf '%s' "$OURS" | grep -vE '^\s*$' | sort)"

NEW="$(comm -13 <(printf '%s\n' "$BASE") <(printf '%s\n' "$NOW"))"
GONE="$(comm -23 <(printf '%s\n' "$BASE") <(printf '%s\n' "$NOW"))"

if [ -n "$NEW" ]; then
	echo "vocab-lint: FAIL — new application-tier vocabulary debt naming $SEAT:"
	printf '%s\n' "$NEW" | sed 's/^/  + /'
	echo
	echo "  implemented-undeclared = we emit a type tag no spec declares. Route it to arch"
	echo "                           (the field ahead of the fold is fine; an unfolded tag is not)."
	echo "  single-seat            = only we emit a declared tag. The convention's whole point is"
	echo "                           that two front-ends produce byte-compatible entities."
	echo "  divergent-family       = the seats emit different tags under one convention. This is"
	echo "                           the one that produces a correct, complete, EMPTY answer."
	echo
	echo "  Fix it, or lower the baseline in the same commit with the reason."
	exit 1
fi

if [ -n "$GONE" ]; then
	echo "vocab-lint: FAIL — baseline rows are no longer reported, so the ratchet must come down:"
	printf '%s\n' "$GONE" | sed 's/^/  - /'
	echo "  Remove them from $BASELINE in this commit. A baseline that outlives its debt"
	echo "  silently re-admits it."
	exit 1
fi

echo "vocab-lint: OK — $CONTEXT; $(printf '%s' "$NOW" | grep -c . ) finding(s) naming $SEAT, matching the baseline"
exit 0

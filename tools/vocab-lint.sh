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
# ⛔ IT RESOLVES OUR SEAT BY DIRECTORY NAME, WHICH IS NOT THIS CHECKOUT WHEN YOU
# ARE IN A WORKTREE — SO WE CONSTRUCT THE PARENT RATHER THAN SKIPPING.
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
# That was answered with a loud SKIP until 2026-09-16, and a loud skip is still a
# class going unchecked: every branch in this repo is developed in a worktree, so
# the one gate no test here can stand in for ran on **no** branch work, ever.
#
# It is CONSTRUCTED now, which is `CORE_RUST_REF`'s move one tool over — build the
# world in which the name resolves to the tree you mean, rather than verifying
# that it happens to. A scratch parent holds a corpus of symlinks (so
# `corpus.parent` is ours to choose), the sibling seats symlinked through, and
# `entity-browser-rust` materialized from THIS checkout.
#
# ⛔ AND THE SEAT IS MATERIALIZED, NOT SYMLINKED, BECAUSE THE ANALYZER'S BUILD-
# OUTPUT SKIP NEVER FIRES ON A TOP-LEVEL `target/`.
#
# Its skip patterns are anchored with slashes on both sides (`/target/`, `/.git/`)
# and are tested with `in` against a path that is RELATIVE to the seat root — so
# `target/debug/…` has no leading slash and never matches. Measured 2026-09-16
# across both worktrees: **0 files skipped here, and 1962 build-output files
# scanned as our source in the main worktree** (stale `.core-pin/` kernel exports
# plus `target-pin-*`).
#
# That is not abstract. `single-seat app/user` sat in our baseline for days, and
# the file that produced it is
# `.core-pin/<kernel-sha>/extensions/compute/src/eval/tests.rs` — **the kernel's
# own test file**, in five stale pin directories, attributed to our commit. Our
# real occurrences (`views/query_console/`) are correctly classed test-only and
# name us nowhere. The baseline's own note had reasoned it out as a doc-example
# false positive; the cause was residue on disk, and only instrumenting the scan
# said so.
#
# So the seat we hand the analyzer is **what git considers part of this repo** —
# tracked plus untracked-not-ignored. Residue is ignored by definition, so this
# needs no list of directories to exclude and cannot rot as new build dirs appear
# (`target-pin-*` did not exist when `/target/` was written). A file you have
# created and not yet added is still yours and is still read.
#
# The analyzer's own JSON carries `head`/`dirty` per seat; the materialized tree
# has no `.git`, so those are read here, from the real checkout, and printed on
# every run — a report that cannot say which tree it is about is the defect this
# whole block exists to answer.
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

here_real="$(cd "$HERE" && pwd -P)"
command -v git >/dev/null 2>&1 || skip "no git — the seat is built from what git considers part of this repo"

# THE SCRATCH PARENT. `corpus.parent` is where the analyzer looks for seats, so
# owning the parent is what lets us say which tree `entity-browser-rust` means.
SHADOW="$(mktemp -d "${TMPDIR:-/tmp}/vocab-lint.XXXXXX")" || skip "could not create a scratch parent"
trap 'rm -rf "$SHADOW"' EXIT
SHADOW_CORPUS="$SHADOW/$(basename "$ARCH")"

# The corpus is a real directory of symlinks: `--root` is `.resolve()`d, so a
# symlinked corpus would resolve straight back to the real parent and undo the
# whole construction. One level down (`corpus/specs`) is a symlink, and an rglob
# rooted AT a symlink does descend — which is why this works where a symlinked
# seat would not. `declared_total` is asserted below so that if the analyzer ever
# walks the corpus differently, this fails loudly instead of reporting a tier in
# which nothing is declared and everything we emit looks invented.
mkdir -p "$SHADOW_CORPUS" || skip "could not build the scratch corpus"
for e in "$ARCH"/* "$ARCH"/.[!.]*; do
	[ -e "$e" ] || continue
	ln -s "$e" "$SHADOW_CORPUS/$(basename "$e")" 2>/dev/null
done
# Every sibling of ours, symlinked: the analyzer resolves a seat before scanning
# it, so the other seats are read exactly as they are on disk. Only OUR seat is
# replaced, and only because the residue problem above is ours to answer for.
for e in "$PARENT"/*; do
	[ -d "$e" ] || continue
	[ "$(basename "$e")" = "$SEAT" ] && continue
	[ "$(basename "$e")" = "$(basename "$ARCH")" ] && continue
	ln -s "$e" "$SHADOW/$(basename "$e")" 2>/dev/null
done

# OUR SEAT: what git considers part of this repo, materialized. See the header —
# the analyzer's `/target/` skip does not fire on a top-level `target/`, and a
# symlinked seat is `.resolve()`d back to the real directory, residue and all.
mkdir -p "$SHADOW/$SEAT" || skip "could not build the scratch seat"
if ! git -C "$HERE" ls-files --cached --others --exclude-standard -z \
	| tar -C "$HERE" --null -T - -cf - 2>/dev/null \
	| tar -C "$SHADOW/$SEAT" -xf - 2>/dev/null; then
	skip "could not materialize this checkout as the seat"
fi

SEAT_HEAD="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo '?')"
[ -n "$(git -C "$HERE" status --short 2>/dev/null)" ] \
	&& SEAT_HEAD="$SEAT_HEAD, DIRTY — this run is not reproducible"

VOCAB_RAW="$(cd "$SHADOW_CORPUS" && python3 "$TOOLS/spec-tool/cli.py" vocab --root "$SHADOW_CORPUS" --json 2>/dev/null)"
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

# ANTI-VACUITY ON THE RIG, NOT ON THE SUBJECT. We hand the analyzer a corpus of
# symlinks, which works because an rglob rooted at a symlink descends. If that
# ever stops holding, the corpus reads as empty — and an empty corpus does not go
# quiet, it declares nothing, which makes EVERY tag we emit `implemented-
# undeclared` and floods the gate with rows that are an artifact of the rig.
# The floor is 1: did the corpus load at all. It must not be set anywhere near
# the real count, or it starts adjudicating the finding instead of guarding the
# instrument.
if not (d.get("declared_total") or 0):
    print("ERR\tcorpus-read-nothing"); sys.exit(0)

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
# WHICH TREE, AT WHICH HEAD, AND HOW MUCH OF IT WAS READ. The head comes from
# the real checkout (the materialized seat has no `.git`), and the file count is
# printed because it is the one number that would have exposed the residue
# defect on sight: 409 files here against 2298 in a sibling worktree of the same
# repo is not a difference between branches, it is a scan reading somebody
# else's tree.
mine = next((s for s in seats if s.get("name") == seat), {})
print("CTX\t%d seat(s), %d missing, %s declared / %s emitted, %s file(s) read" % (
    n_seats, n_missing, d.get("declared_total"), d.get("implemented_total"),
    mine.get("files", "?")))
for c, t in sorted(rows):
    print("ROW\t%s\t%s" % (c, t))
PY
)"

case "$REDUCED" in
	*"ERR"$'\t'"not-json"*) skip "spec vocab did not return JSON (exit $rc)" ;;
	*"ERR"$'\t'"fewer-than-two-seats"*)
		skip "spec vocab found fewer than 2 application seats — is $PARENT/entity-workbench-go present?" ;;
	*"ERR"$'\t'"corpus-read-nothing"*)
		skip "the corpus read through the scratch parent declared NOTHING — the construction in this script is broken, not the tier" ;;
esac

CONTEXT="$(printf '%s\n' "$REDUCED" | sed -n 's/^CTX\t//p')"
OURS="$(printf '%s\n' "$REDUCED" | sed -n 's/^ROW\t//p')"
# `$here_real` is honest because the seat handed to the analyzer was BUILT from
# this checkout — not because a path happened to line up. That is the difference
# between construction and verification, and it is why this line can name a
# worktree at all.
echo "vocab-lint: read $SEAT at $here_real ($SEAT_HEAD)"

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

#!/usr/bin/env bash
# tree-hygiene.sh — refuse a tracked file that .gitignore says should not be.
#
# WHY THIS EXISTS, AND WHY IT IS ONE COMMAND RATHER THAN A LIST OF PATHS:
# `.gitignore` NEVER APPLIES TO AN ALREADY-TRACKED PATH. So the moment a build
# output is committed once — by an over-broad `git add -A`, by a wildcard, by
# accident — the ignore rule that was supposed to cover it goes silently inert,
# `git status` reads clean, and `git check-ignore` reports the path as NOT
# IGNORED (it skips tracked paths by default), which reads as a missing rule
# rather than as a tracked artifact. Every signal points away from the problem.
#
# That has now happened twice in this repo, and the second time was the fix for
# the first:
#   - a 2026-08-21 chat fix swept in 18,408 files / 972,606 insertions —
#     nine rendered publish trees plus a frozen SPA shell.
#   - its follow-up untracked ONE of them (`dist-all/`) and ratcheted `.gitignore`
#     from an enumeration to `/dist-*`, which was the right rule and could not
#     help: the other EIGHT were already tracked, so the new glob never reached
#     them. 12,488 files / 354 MB stayed in the tree, reported as cleaned.
#
# The invariant is therefore not "do not commit dist-*" — a path list has to be
# maintained, and the next artifact directory inherits the unsafe default the
# same way. It is the general form: *the ignore file and the index must agree*.
# `git ls-files -c -i --exclude-standard` is exactly that question, and it needs
# no list to keep current.
#
# Mutation-checked: staging a single ignored file back makes this exit 1 naming
# that file; removing it from the index makes it exit 0.
set -euo pipefail

offenders="$(git ls-files -c -i --exclude-standard)"

if [ -z "$offenders" ]; then
  echo "tree-hygiene: ok — no tracked path is covered by .gitignore"
  exit 0
fi

count="$(printf '%s\n' "$offenders" | grep -c .)"

echo "tree-hygiene: FAIL — $count tracked file(s) are covered by .gitignore." >&2
echo >&2
echo "These are tracked, so the ignore rule covering them does nothing and" >&2
echo "'git status' will keep reading clean. They are almost certainly build" >&2
echo "output committed by accident." >&2
echo >&2
# Roll up to the top-level directory: a publish tree is thousands of files and
# the useful unit is the directory, not the list.
printf '%s\n' "$offenders" | cut -d/ -f1 | sort | uniq -c | sort -rn | \
  awk '{printf "  %-44s %s file(s)\n", $2, $1}' >&2
echo >&2
echo "To untrack them WITHOUT deleting them from disk (they may be served):" >&2
echo "  git rm -r --cached <dir>" >&2
echo >&2
echo "Deleting them instead is a separate decision — check for live servers first." >&2
exit 1

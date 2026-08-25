#!/usr/bin/env bash
# build-stamp — record WHICH COMMIT BUILT THIS SHELL, in the shell itself.
#
# An emitted tree carried nothing that named the commit its wasm came from, so
# reconstructing a cut's provenance meant diffing file mtimes against `git log`.
# That is archaeology, and it cost a session upstream: a cut was hunted for app
# bugs that had been fixed 62 commits earlier, because nothing on the tree said
# how old the app in it was.
#
# **The stamp goes in `index.html`, and that placement is the whole design.**
# The alternative — `/entity-deployment.json` — is written by the *publish*
# binary, not the frontend build, and those are not always the same commit: a
# pipeline that reuses a previous cut's shell (meta's `--spa-from`) copies the
# seven shell files and republishes the content around them. Stamping the
# deployment config would then name the commit of the publisher while the app
# beside it came from somewhere else, which is worse than no stamp — it would be
# a confident wrong answer to exactly the question being asked. `index.html` is
# one of those seven files, so the stamp travels with the code it describes.
#
# **No timestamp, deliberately.** The same commit must produce the same bytes;
# a build clock in the output would make every cut differ from every other and
# destroy the reproducibility this is meant to support. The commit and whether
# the tree was dirty are the two facts that actually vary.
#
# Runs in the build image (git is present there and the repo is bind-mounted).
# It never fails a build: an unresolvable commit stamps `unknown` rather than
# stopping a release, because a missing provenance string is a smaller problem
# than a build that will not produce an artifact at all.
set -euo pipefail

dist="${1:?usage: build-stamp.sh <dist-dir>}"
html="$dist/index.html"

if [ ! -f "$html" ]; then
    echo "build-stamp: no $html — nothing to stamp (did trunk write elsewhere?)" >&2
    exit 0
fi

# `git` may be absent, the tree may not be a checkout, or the bind mount may
# trip git's ownership check. Every one of those is "we do not know", not a
# build failure.
commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
if [ "$commit" != "unknown" ] && ! git diff --quiet HEAD 2>/dev/null; then
    # A dirty build is a real and important distinction for a release artifact:
    # the commit alone would claim a provenance the bytes do not have.
    commit="$commit-dirty"
fi

# Idempotent: trunk regenerates index.html on every build, but a re-stamp of an
# already-stamped file (a hand-run, a reused shell) must replace rather than
# accumulate.
python3 - "$html" "$commit" <<'PY'
import re, sys

path, commit = sys.argv[1], sys.argv[2]
with open(path, encoding='utf-8') as fh:
    html = fh.read()

tag = f'<meta name="entity-build" content="{commit}">'
existing = re.compile(r'<meta name="entity-build" content="[^"]*">')
if existing.search(html):
    html = existing.sub(tag, html, count=1)
elif '<head>' in html:
    html = html.replace('<head>', '<head>' + tag, 1)
else:
    # No <head> to hang it on. Say so rather than writing the tag somewhere it
    # does not belong — a stamp in the wrong place is a stamp nobody finds.
    sys.stderr.write('build-stamp: no <head> in index.html — not stamped\n')
    sys.exit(0)

with open(path, 'w', encoding='utf-8') as fh:
    fh.write(html)
PY

echo "build-stamp: $dist/index.html ← $commit"

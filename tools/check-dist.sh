#!/usr/bin/env bash
# Post-build deploy-staleness guard.
#
# Catches the failure class that once bricked the app and that
# `make e2e-worker` STRUCTURALLY cannot (fresh container per run, no SW cache
# across two builds): `dist/index.html` references hashed bundles that don't
# exist on disk, or assets that are empty / truncated. A green `dist/` listing
# is not a working deploy — this verifies the artifact set is internally
# consistent before we serve it.
#
# Asserts, against dist/:
#   1. index.html + sw.js + the worker loader exist and are non-empty.
#   2. EVERY entity-browser-<hash>.{js,wasm} and entity-worker*.{js,wasm}
#      referenced by index.html exists on disk and is non-empty.
#
# Exit 0 = consistent; non-zero with a clear message = stale/broken.
# Wired into `make wasm` / `make wasm-release` so a bad build fails loudly.
set -euo pipefail

DIST="${1:-dist}"
INDEX="$DIST/index.html"
fail=0

err() { printf '  ✗ %s\n' "$1" >&2; fail=1; }

if [ ! -f "$INDEX" ]; then
    echo "check-dist: $INDEX not found — did the build run?" >&2
    exit 1
fi

# 1. Core shell files must exist and be non-empty.
for f in index.html sw.js; do
    if [ ! -s "$DIST/$f" ]; then err "$f missing or empty"; fi
done
# The worker bundle is loaded at runtime by the loader (not <link>'d in
# index.html), so the refs scan below won't see it — but it's the OTHER
# staleness vector (non-hashed, same URL across builds). If the loader is
# present (worker mode built), its bundle must be present + non-empty too.
if [ -f "$DIST/entity-worker-loader.js" ]; then
    [ -s "$DIST/entity-worker-loader.js" ] || err "entity-worker-loader.js is empty"
    for wf in entity-worker.js entity-worker_bg.wasm; do
        if [ ! -s "$DIST/$wf" ]; then err "$wf missing or empty (worker loader present)"; fi
    done
fi

# 2. Every hashed/worker asset referenced by index.html must exist + be non-empty.
#    Trunk emits `entity-browser-<hash>.js`, `..._bg.wasm`, and the non-hashed
#    `entity-worker*.{js,wasm}`. Pull every such reference out of index.html.
refs=$(grep -oE 'entity-(browser-[0-9a-f]+|worker)(_bg|-loader)?\.(js|wasm)' "$INDEX" | sort -u || true)

if [ -z "$refs" ]; then
    err "index.html references no entity-* bundle — build output looks wrong"
fi

for ref in $refs; do
    if [ ! -e "$DIST/$ref" ]; then
        err "index.html references $ref but it is MISSING from $DIST/ (stale build?)"
    elif [ ! -s "$DIST/$ref" ]; then
        err "$ref exists but is EMPTY (truncated build?)"
    fi
done

if [ "$fail" -ne 0 ]; then
    echo "check-dist: $DIST/ is INCONSISTENT — do not serve/deploy this build." >&2
    exit 1
fi

echo "check-dist: $DIST/ consistent ($(echo "$refs" | wc -w | tr -d ' ') referenced assets present)."

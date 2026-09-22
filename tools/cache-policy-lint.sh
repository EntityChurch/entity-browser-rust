#!/usr/bin/env bash
# C15 — pin the PYTHON expression of the cache-immutability rule to the same
# vectors the Rust one is pinned to.
#
# The rule is expressed four times in three languages (see
# `tools/cache-policy-vectors.txt`). Rust's two call sites share one file and
# both run the vectors under `make test` / `make test-tauri`. This is the third:
# `tools/cors-serve.py`, which is BOTH our dev/serve path and the reference
# implementation `PUBLISHING-QUICKSTART` §6.1 tells operators to copy into a CDN
# — so a divergence here is a divergence in what we ship AND in what we advise.
#
# Why a gate and not a comment: the previous arrangement was a comment in each
# file saying it mirrored the others, and `GOTCHAS.md` asserting they therefore
# could not disagree. `REVIEW-2026-08-25` §2.1 found all four disagreeing.
#
# The fourth expression — §6.2's Cloudflare recipe — is prose and cannot be
# executed. `tools/cache-policy-doc-check.py` holds it to the same regexes by
# grepping the document for them, which is the most a doc can be held to.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import importlib.util
import sys

spec = importlib.util.spec_from_file_location("cors_serve", "tools/cors-serve.py")
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

vectors = []
with open("tools/cache-policy-vectors.txt", encoding="utf-8") as fh:
    for raw in fh:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        expect, path = line.split(None, 1)
        if expect not in ("immutable", "mutable"):
            sys.exit(f"cache-policy-lint: bad expectation {expect!r} in vectors")
        vectors.append((expect == "immutable", path.strip()))

if len(vectors) < 20:
    sys.exit(
        f"cache-policy-lint: only {len(vectors)} vectors — this file is the shared "
        "specification for four expressions of the rule, and a shrinking one weakens "
        "every gate that reads it at once."
    )

wrong = [
    f"  {path!r}: expected {'immutable' if want else 'mutable'}, "
    f"got {'immutable' if mod.is_immutable(path) else 'mutable'}"
    for want, path in vectors
    if mod.is_immutable(path) != want
]

if wrong:
    print("cache-policy-lint: FAIL — tools/cors-serve.py disagrees with the shared vectors")
    print("\n".join(wrong))
    print()
    print("This file is what PUBLISHING-QUICKSTART §6.1 tells operators to copy into a")
    print("CDN config, so a divergence here ships to every deployment that follows the")
    print("documentation. Fix the rule, or add the vector if the rule is right.")
    sys.exit(1)

both = sum(1 for w, _ in vectors if w), sum(1 for w, _ in vectors if not w)
print(
    f"cache-policy-lint: OK — cors-serve.py matches all {len(vectors)} shared vectors "
    f"({both[0]} immutable / {both[1]} mutable)"
)
PY

python3 tools/cache-policy-doc-check.py

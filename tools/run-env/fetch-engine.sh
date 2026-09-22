#!/usr/bin/env bash
# fetch-engine.sh -- fetch the vendored engine + terminal, and VERIFY them.
#
#   ./fetch-engine.sh            # fetch into ./v86-m1/, verify every artifact
#   ./fetch-engine.sh --check    # verify what is already here, fetch nothing
#
# Exit 0 iff every artifact is present AND matches its pinned hash.
#
# WHY THIS IS A SCRIPT AND NOT A DIRECTORY OF COMMITTED BINARIES
#
# 9.9 MB of minified blobs in a source tree is 9.9 MB nobody can review. These
# are all re-fetchable and every one is pinned by content below, so the honest
# artifact is the PIN, not the bytes.
#
# WHY THE HASHES ARE THE VERSION
#
# Layer 6 (xterm) pins to an immutable npm version. Layer 1 (v86) CANNOT:
# `libv86.js` carries no version string, copy.sh is a live URL with no version
# in the path, and v86's only GitHub release is a rolling `latest` tag whose
# bytes are a DIFFERENT BUILD from what copy.sh serves (v86.wasm 2.10 MB there
# against the 1.42 MB we measured). So there is no upstream version to record
# and the sha256 is the pin. Established by hash-matching every row against its
# named upstream on 2026-09-11 -- see
# docs/plans/MEASUREMENT-2026-09-11-r-THE-PROVENANCE-OF-SEVEN-LAYERS-*.md
#
# ⚠ A MISMATCH IS A RESULT, NOT AN ERROR TO ROUTE AROUND. copy.sh is mutable,
# so one day this will fail. That is the script working: it means upstream moved
# under us and somebody has to decide whether to adopt the new bytes. Do not
# "fix" it by refreshing the constant without re-reading what changed.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${HERE}/v86-m1"
MODE="${1:-fetch}"

# artifact | sha256 | licence | source
read -r -d '' MANIFEST <<'EOF' || true
build/libv86.js|b80fba71dacb7977e5b46800b3ba194bba7fe13e52fa3d22f80cc060ff015a4e|BSD-2-Clause|https://copy.sh/v86/build/libv86.js
build/v86.wasm|abd512988f39f5c81d4930f564bafa1a6e1e0d5210de9b20e2bdec13fbc318d9|BSD-2-Clause|https://copy.sh/v86/build/v86.wasm
bios/seabios.bin|73e3f359102e3a9982c35fce98eb7cd08f18303ac7f1ba6ebfbe6cdc1c244d98|LGPL-3.0|https://copy.sh/v86/bios/seabios.bin
bios/vgabios.bin|a4bc0d80cc3ca028c73dafa8fee396b8d054ce87ebd8abfbd31b06b437607880|LGPL-3.0|https://copy.sh/v86/bios/vgabios.bin
EOF

# npm package | version | path inside tarball | sha256 | destination
read -r -d '' NPM <<'EOF' || true
@xterm/xterm|5.5.0|package/lib/xterm.js|1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495|vendor/xterm.js
@xterm/xterm|5.5.0|package/css/xterm.css|ba8e6985669488981ccf40c0cefe3aba80722cb6c92de7ad628b0bd717faf2b6|vendor/xterm.css
@xterm/addon-fit|0.10.0|package/lib/addon-fit.js|bdaefa370b1bfc42ee88d46fe6072400902a4d4b2d45cd93438dda9b23c97089|vendor/addon-fit.js
EOF

fail=0
# NOTE: every loop below reads from a HERE-STRING, never a pipe. `while ... |`
# runs its body in a SUBSHELL, so `fail` would be set and discarded and this
# script could never exit non-zero -- a verifier that cannot fail is worse than
# no verifier, because it reports success for a tree it never looked at.
check() { # path expected
  local p="$OUT/$1" e="$2" a
  if [ ! -f "$p" ]; then printf "  MISSING  %s\n" "$1"; fail=1; return; fi
  a=$(sha256sum "$p" | cut -d' ' -f1)
  if [ "$a" = "$e" ]; then
    printf "  ok       %s\n" "$1"
  else
    printf "  MISMATCH %s\n     pinned %s\n     got    %s\n" "$1" "$e" "$a"; fail=1
  fi
}

verify_all() {
  while IFS='|' read -r p h l s; do [ -n "${p:-}" ] && check "$p" "$h"; done <<< "$MANIFEST"
  while IFS='|' read -r pkg ver inner h dest; do [ -n "${pkg:-}" ] && check "$dest" "$h"; done <<< "$NPM"
}

if [ "$MODE" = "--check" ]; then
  echo "==> verifying ${OUT}"
  verify_all
  [ "$fail" -eq 0 ] && echo "==> all artifacts match their pins" || echo "==> FAILED"
  exit "$fail"
fi

mkdir -p "$OUT/build" "$OUT/bios" "$OUT/vendor"
echo "==> fetching the engine (v86, copy.sh) and the terminal (xterm, npm)"

while IFS='|' read -r p h l s; do
  [ -z "${p:-}" ] && continue
  curl -sSL -o "$OUT/$p" "$s"
done <<< "$MANIFEST"

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
while IFS='|' read -r pkg ver inner h dest; do
  [ -z "${pkg:-}" ] && continue
  base="${pkg##*/}"
  tgz="$tmp/${base}-${ver}.tgz"
  [ -f "$tgz" ] || curl -sSL -o "$tgz" "https://registry.npmjs.org/${pkg}/-/${base}-${ver}.tgz"
  tar xzOf "$tgz" "$inner" > "$OUT/$dest"
done <<< "$NPM"

verify_all
if [ "$fail" -ne 0 ]; then
  echo "==> FAILED -- upstream does not match the pin. Read the entry above before"
  echo "    touching the constants: copy.sh is mutable and this is how you find out."
  exit 1
fi
echo "==> all artifacts match their pins"

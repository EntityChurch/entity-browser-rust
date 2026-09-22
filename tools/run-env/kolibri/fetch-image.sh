#!/usr/bin/env bash
# fetch-image.sh -- the KolibriOS floppy image this app boots, VERIFIED.
#
#   ./fetch-image.sh           # fetch into ./guest/, verify
#   ./fetch-image.sh --check   # verify what is here, fetch nothing
#
# The image is KolibriOS's own CI build (the one kolibrios.org/en/download links),
# pinned by sha256 exactly as fetch-engine.sh pins v86. The CI directory is keyed
# by build (0.7.7.0-9197-gb0055ba47), and upstream publishes a sha256sums.txt beside
# it; the pin below matched that file when it was taken (2026-09-14). The `g…` in
# the build name is the git commit of git.kolibrios.org/KolibriOS/kolibrios the
# image was built from -- which is where its GPLv2 source is.
#
# The page boots guest/kolibri.img, which build-image.sh makes from this one
# (plus our absolute-pointer driver). This file is upstream's bytes, untouched.
#
# ⚠ A MISMATCH IS A RESULT: upstream moved, and someone decides whether to adopt it.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BUILD="0.7.7.0-9197-gb0055ba47"
URL="https://builds.kolibrios.org/ci/${BUILD}/en_US/kolibrios-${BUILD}-en_US.img"
SHA="2c1216b6e697520715e598bc9d1e0cbbfb44c3d2ced7c6927b5731f2dc416c1f"
OUT="$HERE/guest/kolibri-upstream.img"
mkdir -p "$HERE/guest"
if [ "${1:-}" != "--check" ] && [ ! -f "$OUT" ]; then
  echo "==> fetching $URL"
  curl -fsSL -o "$OUT.part" "$URL" && mv "$OUT.part" "$OUT"
fi
[ -f "$OUT" ] || { echo "  MISSING  guest/kolibri-upstream.img"; exit 1; }
got=$(sha256sum "$OUT" | cut -d' ' -f1)
if [ "$got" = "$SHA" ]; then
  echo "  ok       guest/kolibri-upstream.img ($(stat -c %s "$OUT") bytes, KolibriOS ${BUILD})"
else
  printf "  MISMATCH guest/kolibri-upstream.img\n     pinned %s\n     got    %s\n" "$SHA" "$got"; exit 1
fi
# The page is served standalone from this directory, so it reaches the engine and
# the SDK through the same relative links the Alpine page uses.
ln -sfn ../v86-m1/build "$HERE/build"
ln -sfn ../v86-m1/bios  "$HERE/bios"

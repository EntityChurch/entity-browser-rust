#!/usr/bin/env bash
# package-app.sh -- package the Alpine Environment as an entity-app `dist/`.
#
#   ./package-app.sh [outdir]                  default: ./app-dist
#   ./package-app.sh <outdir> --onto <dist>    Alpine ADDED to an existing entity-apps dist/
#
# --onto EXISTS BECAUSE `--ingest-apps` IS THE WHOLE APP SET, NOT AN ADDITION.
# A publish replaces every app under {peer}/apps/** with what that one directory
# holds, so publishing ./app-dist alone to a domain that serves entity-apps'
# catalog REMOVES those apps from it. --onto builds one directory holding both:
# the base's files hardlinked in, index.json = the base's entries plus ours (an
# `alpine` entry already in the base is replaced, and it says so). The base is
# only read. A stopgap until the app lives in entity-apps itself.
#
# Produces the shape `publish --ingest-apps` (and `make site APPS_DIST=…`) reads:
#
#   <out>/index.json                     one catalog entry, declaring two bundles
#   <out>/alpine.html                    the page, with xterm + libv86 INLINED
#   <out>/alpine.assets/engine/          v86.wasm · seabios.bin · vgabios.bin
#   <out>/alpine.assets/image/           vmlinuz · initramfs · fs.json · blobs/ · snapshot.{bin.zst,json}?
#   <out>/alpine.assets/packages/        fs.json · blobs/   (only if build-packages.sh ran)
#
# WHY SEPARATE BUNDLES: they change for different reasons and on different clocks.
# The engine moves with OUR builds; the image moves when someone rebuilds or
# re-profiles the environment (fish instead of bash, a toolchain, a package
# set). A republish of one must not re-address the other, and the index-per-
# bundle design (src/apps/assets.rs) means it does not.
#
# WHY INLINE THE SCRIPTS: an entity-app bundle is one self-contained HTML file
# (entity-apps' build.py concatenates <script> tags for the same reason). The
# ~650 KB of JS is code and belongs in the bundle; the ~33 MB of binaries are
# data, faulted on demand, and belong in the asset bundles.
#
# FILES ARE HARDLINKED, NOT SYMLINKED: `make site` stages APPS_DIST with `cp -r`
# into a container that mounts only this repo, and a symlink to a path outside
# that mount (or an absolute one) arrives dangling. Falls back to a copy across
# filesystems.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT_ARG="" ONTO=""
while [ $# -gt 0 ]; do
  case "$1" in
    --onto) ONTO="${2:?--onto needs a directory}"; shift 2 ;;
    --onto=*) ONTO="${1#--onto=}"; shift ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) OUT_ARG="$1"; shift ;;
  esac
done
if [ -n "$ONTO" ]; then
  [ -f "$ONTO/index.json" ] || { echo "--onto $ONTO: no index.json -- not an entity-apps dist/" >&2; exit 1; }
  [ -n "$OUT_ARG" ] || { echo "--onto needs an explicit output directory" >&2; exit 2; }
  ONTO="$(cd "$ONTO" && pwd)"
fi
OUT="$(mkdir -p "${OUT_ARG:-$HERE/app-dist}" && cd "${OUT_ARG:-$HERE/app-dist}" && pwd)"
GUEST="$HERE/alpine-guest"
ENGINE="$HERE/v86-m1"

for f in "$GUEST/guest/vmlinuz" "$GUEST/guest/initramfs" "$GUEST/guest/fs.json" \
         "$ENGINE/build/v86.wasm" "$ENGINE/build/libv86.js" "$ENGINE/bios/seabios.bin" \
         "$ENGINE/bios/vgabios.bin" "$ENGINE/vendor/xterm.js" "$ENGINE/vendor/addon-fit.js" \
         "$ENGINE/vendor/xterm.css"; do
  [ -f "$f" ] || { echo "missing $f -- run ./fetch-engine.sh and ./build-guest.sh first" >&2; exit 1; }
done

if [ -n "$ONTO" ]; then
  [ "$ONTO" != "$OUT" ] || { echo "--onto and the output are the same directory; the base is read-only" >&2; exit 1; }
  # A merged output must not carry a previous base's apps forward, so it is
  # emptied -- but only if this script made it (marker) or it is already empty.
  if [ -n "$(ls -A "$OUT")" ] && [ ! -f "$OUT/.package-app" ]; then
    echo "$OUT is not empty and was not made by package-app.sh --onto; refusing to clear it" >&2; exit 1
  fi
  find "$OUT" -mindepth 1 -delete
  touch "$OUT/.package-app"
  # Everything the base ships except its catalog (merged below) and any Alpine of its own.
  for f in "$ONTO"/*; do
    case "$(basename "$f")" in index.json|alpine.html|alpine.assets) continue ;; esac
    cp -rl "$f" "$OUT/" 2>/dev/null || cp -r "$f" "$OUT/"
  done
else
  rm -rf "$OUT/alpine.assets" "$OUT/alpine.html" "$OUT/index.json"
fi
mkdir -p "$OUT/alpine.assets/engine" "$OUT/alpine.assets/image"

place() { ln -f "$1" "$2" 2>/dev/null || cp "$1" "$2"; }
# The package set is optional: without it the catalog does not declare the
# bundle, and the guest's apk says its repository is missing.
BUNDLES='"engine", "image"'
if [ -f "$GUEST/guest/packages/fs.json" ]; then
  mkdir -p "$OUT/alpine.assets/packages"
  place "$GUEST/guest/packages/fs.json" "$OUT/alpine.assets/packages/fs.json"
  cp -rl "$GUEST/guest/packages/blobs" "$OUT/alpine.assets/packages/blobs" 2>/dev/null \
    || cp -r "$GUEST/guest/packages/blobs" "$OUT/alpine.assets/packages/blobs"
  BUNDLES='"engine", "image", "packages"'
else
  echo "    no package set ($GUEST/guest/packages) -- run ./build-packages.sh to include one"
fi
place "$ENGINE/build/v86.wasm"    "$OUT/alpine.assets/engine/v86.wasm"
place "$ENGINE/bios/seabios.bin"  "$OUT/alpine.assets/engine/seabios.bin"
place "$ENGINE/bios/vgabios.bin"  "$OUT/alpine.assets/engine/vgabios.bin"
for f in vmlinuz initramfs fs.json; do place "$GUEST/guest/$f" "$OUT/alpine.assets/image/$f"; done
# The snapshot (build-snapshot.py) resumes the machine instead of booting it. The
# page checks it fits this image before using it, so shipping a stale one costs a
# download and a cold boot, never a broken machine.
if [ -f "$GUEST/guest/snapshot.bin.zst" ] && [ -f "$GUEST/guest/snapshot.json" ]; then
  place "$GUEST/guest/snapshot.bin.zst" "$OUT/alpine.assets/image/snapshot.bin.zst"
  place "$GUEST/guest/snapshot.json"    "$OUT/alpine.assets/image/snapshot.json"
else
  echo "    no snapshot ($GUEST/guest/snapshot.bin.zst) -- launches boot cold; see build-snapshot.py"
fi
cp -rl "$GUEST/guest/blobs" "$OUT/alpine.assets/image/blobs" 2>/dev/null \
  || cp -r "$GUEST/guest/blobs" "$OUT/alpine.assets/image/blobs"

python3 - "$GUEST/index.html" "$ENGINE" "$OUT/alpine.html" <<'PY'
import sys, pathlib
page, engine, out = pathlib.Path(sys.argv[1]).read_text(), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
def script(rel):
    body = (engine / rel).read_text()
    # A literal </script> inside an inlined script ends the element early.
    assert "</script" not in body.lower(), f"{rel} contains </script>; cannot inline safely"
    return "<script>\n" + body + "\n</script>"
swaps = {
    '<link rel="stylesheet" href="vendor/xterm.css">': "<style>\n" + (engine / "vendor/xterm.css").read_text() + "\n</style>",
    '<script src="vendor/xterm.js"></script>': script("vendor/xterm.js"),
    '<script src="vendor/addon-fit.js"></script>': script("vendor/addon-fit.js"),
    '<script src="build/libv86.js"></script>': script("build/libv86.js"),
}
for tag, repl in swaps.items():
    assert page.count(tag) == 1, f"expected exactly one {tag!r} in the page -- it moved, re-read it"
    page = page.replace(tag, repl)
out.write_text(page)
print(f"    alpine.html {len(page.encode()):,} bytes (scripts inlined)")
PY

cat > "$OUT/index.json" <<JSON
[
  {
    "id": "alpine",
    "name": "Alpine Linux",
    "description": "A real Linux machine in a tab: Alpine 3.21 with bash, on v86. Files move in and out with send and receive; apk add installs tools.",
    "saves": false,
    "type": "tool",
    "category": "developer",
    "glyph": "🐧",
    "x-files": true,
    "x-assets": [$BUNDLES],
    "x-workspace": true
  }
]
JSON

if [ -n "$ONTO" ]; then
  python3 - "$OUT/index.json" "$ONTO/index.json" <<'MERGE'
import json, sys
ours = json.load(open(sys.argv[1]))
base = json.load(open(sys.argv[2]))
assert isinstance(base, list), "the base index.json must be a JSON array"
ids = {e["id"] for e in ours}
if any(e.get("id") in ids for e in base):
    print(f"    base already has {sorted(ids & {e.get('id') for e in base})} -- replaced by ours")
merged = [e for e in base if e.get("id") not in ids] + ours
with open(sys.argv[1], "w") as f:
    json.dump(merged, f, indent=2, ensure_ascii=False)
    f.write("\n")
print(f"    catalog: {len(base)} from the base + {len(ours)} = {len(merged)} entries")
MERGE
fi

files=$(find "$OUT/alpine.assets" -type f | wc -l)
bytes=$(du -sbL "$OUT/alpine.assets" | cut -f1)
echo "==> $OUT"
echo "    asset bundles: ${BUNDLES//\"/}, $files files, $((bytes / 1048576)) MiB"
echo "    publish it:    make site-dist APPS_DIST=$OUT   (then make serve DIST=dist-site, open Apps)"

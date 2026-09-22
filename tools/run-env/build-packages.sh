#!/usr/bin/env bash
# build-packages.sh -- the Alpine Environment's package set, as a signed local
# apk repository indexed for v86's 9p filesystem.
#
#   ./build-packages.sh [outdir]       default: ./alpine-guest
#
# Produces  <out>/guest/packages/{fs.json,blobs/}  -- the SAME format as the image
# (fs2json.py), holding one directory, entity-packages/x86/, with the .apk files
# and a signed APKINDEX.tar.gz. package-app.sh ships it as the `packages` asset
# bundle; the page merges it into the guest's root at /var/cache/entity-packages
# and routes reads of those files to that bundle. build-guest.sh points
# /etc/apk/repositories there and trusts the key.
#
# WHY A LOCAL REPOSITORY IN THE 9p TREE, NOT A MIRROR: the guest has no network,
# and it does not need one. v86 fetches a 9p file body only when the guest reads
# it, by content hash, through the page's storage seam. So `apk add vim` reads
# the ~100 KB index and the handful of .apk files vim needs, and nothing else in
# the set is ever downloaded by that visitor. The whole mechanism is "put the
# repository in the filesystem"; the lazy fault does the rest.
#
# WHY A SEPARATE BUNDLE: the package set changes on a different clock from the
# image. With its own index, re-picking packages re-addresses only this bundle,
# and a domain that does not want to host packages publishes without it -- the
# page boots the same image and `apk add` says the repository is missing.
#
# WHAT IS DROPPED: any package the image already ships AT THE SAME VERSION,
# read out of the built guest's own /lib/apk/db/installed. A different version
# is kept, so a dependency that needs the newer one can still be satisfied.
# So build the guest FIRST: the drop is only correct against the image it ships
# beside. It lives INSIDE guest/ for the same reason -- build-guest.sh clears
# guest/, so a rebuilt image cannot keep a package set computed against the old
# one. (It is also where the page looks for it standalone: guest/packages/.)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$(cd "${1:-$HERE/alpine-guest}" && pwd)"
ALPINE="${ALPINE:-3.22}"
IMAGE="docker.io/i386/alpine:${ALPINE}"
LIST="${PACKAGES_LIST:-$HERE/packages.txt}"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1757600000}"

[ -f "$OUT/guest/fs.json" ] || { echo "no built guest at $OUT/guest -- run ./build-guest.sh first" >&2; exit 1; }
PKGS="$(sed 's/#.*//' "$LIST" | tr -s ' \t\r\n' ' ' | sed 's/^ //; s/ $//')"
[ -n "$PKGS" ] || { echo "$LIST names no packages" >&2; exit 1; }
KEYNAME="$("$HERE/package-key.sh")"

PKGOUT="$OUT/guest/packages"
echo "==> building the package set ($(wc -w <<<"$PKGS") names from $(basename "$LIST")) into $PKGOUT"
rm -rf "$PKGOUT"; mkdir -p "$PKGOUT"

podman run --rm --arch 386 --security-opt label=disable \
  -v "$HERE/fs2json.py:/fs2json.py:ro" \
  -v "$HERE/.keys:/keys:ro" \
  -v "$OUT/guest:/guest:ro" \
  -v "$PKGOUT:/packages" \
  -e SOURCE_DATE_EPOCH -e PKGS="$PKGS" -e KEYNAME="$KEYNAME" \
  "$IMAGE" sh -euc '
    sed -i "s/^#//" /etc/apk/repositories
    apk add --no-cache abuild python3 >/dev/null 2>&1

    # ---- 1. what the image already has, read out of the image itself -------
    python3 - <<"PY" > /tmp/installed
import json
fs = json.load(open("/guest/fs.json"))
def find(nodes, parts):
    for n in nodes:
        if n[0] == parts[0]:
            return n if len(parts) == 1 else find(n[6], parts[1:])
    raise SystemExit("the image has no /lib/apk/db/installed -- is this an Alpine guest?")
node = find(fs["fsroot"], ["lib", "apk", "db", "installed"])
name = None
for line in open("/guest/blobs/" + node[6], encoding="utf-8", errors="replace"):
    if line.startswith("P:"): name = line[2:].strip()
    elif line.startswith("V:") and name: print(f"{name}-{line[2:].strip()}.apk"); name = None
PY
    echo "    the image ships $(wc -l < /tmp/installed) packages"

    # ---- 2. the closure, minus what the image has at the same version -------
    mkdir -p /stage/entity-packages/x86 && cd /stage/entity-packages/x86
    apk fetch --no-cache --recursive $PKGS >/tmp/fetch.log 2>&1 \
      || { cat /tmp/fetch.log; echo "apk fetch failed -- a name in the list does not exist for x86?" >&2; exit 1; }
    fetched=$(ls *.apk | wc -l)
    dropped=0
    while read -r f; do [ -f "$f" ] && rm -f "$f" && dropped=$((dropped + 1)); done < /tmp/installed
    echo "    fetched $fetched, dropped $dropped already in the image, kept $(ls *.apk | wc -l) ($(du -sm . | cut -f1) MiB)"

    # ---- 3. the index, signed with our key ---------------------------------
    apk index --no-cache --rewrite-arch x86 -d "Alpine Environment package set" \
        -o APKINDEX.tar.gz *.apk 2>/tmp/index.log || { cat /tmp/index.log; exit 1; }
    cp /keys/entity-packages.rsa "/tmp/$KEYNAME.key"
    abuild-sign -k "/tmp/$KEYNAME.key" -p "$KEYNAME" APKINDEX.tar.gz >/dev/null
    tar -tzf APKINDEX.tar.gz | head -1 | grep -q "^.SIGN.RSA" \
      || { echo "the index did not come out signed" >&2; exit 1; }
    echo "    APKINDEX.tar.gz signed as $KEYNAME"

    # ---- 4. index it the way the image is indexed --------------------------
    find /stage -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
    python3 /fs2json.py /stage /packages
  '

echo "==> $PKGOUT"

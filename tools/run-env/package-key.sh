#!/usr/bin/env bash
# package-key.sh -- the key that signs the Alpine Environment's package index.
#
#   ./package-key.sh          prints the key NAME, creating the key if there is none
#
# Two consumers, one key: build-guest.sh puts the PUBLIC half into the image's
# /etc/apk/keys, and build-packages.sh signs APKINDEX.tar.gz with the PRIVATE
# half. apk then installs from the set without --allow-untrusted: a package
# index nobody signed, or one signed with any other key, is refused.
#
# THE PRIVATE KEY IS NOT IN GIT, and that is a decision with a cost: .keys/ is
# gitignored, so it lives on the machine that built the image. Lose it and the
# next package set cannot be signed for images already published -- the fix is
# a new key, a rebuilt image, and a republish. Where the key lives for a
# release is the operator's call (PLAN-2026-09-13-THE-VM-RELEASE §4, D7).
#
# The name carries a fingerprint of the public key, because apk looks the key
# up BY NAME (.SIGN.RSA.<name> in the index -> /etc/apk/keys/<name>): a rotated
# key gets a new name, and an image that trusts the old one refuses the new
# index loudly instead of verifying against the wrong key.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
KEYS="$HERE/.keys"
mkdir -p "$KEYS"
chmod 0700 "$KEYS"

if [ ! -f "$KEYS/entity-packages.rsa" ]; then
  echo "==> no package-signing key; creating one in $KEYS (gitignored -- back it up)" >&2
  podman run --rm --security-opt label=disable -v "$KEYS:/k" docker.io/i386/alpine:3.21 sh -euc '
    apk add --no-cache openssl >/dev/null 2>&1
    umask 077
    openssl genrsa -out /k/entity-packages.rsa 2048 2>/dev/null
    openssl rsa -in /k/entity-packages.rsa -pubout -out /k/entity-packages.pub 2>/dev/null
    chmod 0644 /k/entity-packages.pub
  ' >&2
fi

fp=$(sha256sum "$KEYS/entity-packages.pub" | cut -c1-8)
name="entity-packages-${fp}.rsa.pub"
[ -f "$KEYS/$name" ] || cp "$KEYS/entity-packages.pub" "$KEYS/$name"
echo "$name"

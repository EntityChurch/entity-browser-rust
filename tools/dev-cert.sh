#!/usr/bin/env bash
# dev-cert — mint a locally-trusted TLS cert so `make serve` can be HTTPS.
#
# **Why this exists, and why a plain self-signed cert is not enough.**
#
# Service workers require a SECURE CONTEXT. The only plain-HTTP exceptions are
# `localhost` and `127.0.0.1`. So a dev build reached from a phone or a second
# machine — i.e. by LAN IP, `http://192.168.x.x:8081` — registers NO service
# worker, stores nothing, and has NO offline capability whatsoever. Every
# service-worker behaviour in this repo (the shell cache, the offline boot, the
# build-scoped worker key) is therefore unreachable from any device but the
# build machine, which is a whole tier the harness cannot test.
#
# The obvious fix — a self-signed cert — **does not work in Chrome.** Clicking
# through the interstitial is not sufficient: the origin still carries a
# certificate error and registration fails with
#   SecurityError: Failed to register a ServiceWorker: An SSL certificate error
#   occurred when fetching the script.
# Firefox is more lenient and honours a manually-added exception, so a naive
# self-signed setup "works" on one browser and not the other — which is worse
# than not working at all, because it reads as a browser bug.
#
# What actually works is a certificate the device TRUSTS. So this mints a small
# local CA once, signs a leaf for localhost + this machine's LAN addresses, and
# tells you to install the CA on whatever device you are testing from. That is
# what `mkcert` does; we do it with the openssl already in the build image
# rather than adding a toolchain (AGENTS-STANDARD: system toolchains, minimal
# dependencies).
#
# The CA is generated ONCE and reused, so a device only ever installs it once,
# even as the leaf is re-minted for new addresses.
#
#     tools/dev-cert.sh [extra-san ...]
#
# Extra SANs may be DNS names or IPs; they are classified automatically.
# Output (gitignored): .dev-certs/{ca.crt,ca.key,dev.crt,dev.key}
#
# THESE ARE DEVELOPMENT CREDENTIALS. The CA key sits unencrypted on disk and
# anyone holding it can mint a certificate your test device will trust. It is
# gitignored, it must never leave this machine, and it must never be installed
# on a device you care about. `rm -rf .dev-certs` revokes the whole thing —
# after which remove the CA from any device you installed it on.
set -euo pipefail

out="${DEV_CERT_DIR:-.dev-certs}"
mkdir -p "$out"
chmod 700 "$out"

ca_crt="$out/ca.crt"
ca_key="$out/ca.key"
leaf_crt="$out/dev.crt"
leaf_key="$out/dev.key"

# --- collect the names the cert must cover -----------------------------------
# A browser matches the URL's host against the SANs. `localhost` alone is
# useless here: the entire point is the LAN address. CN is ignored by every
# modern browser, so SANs are the whole of it.
dns=(localhost)
ips=(127.0.0.1 ::1)

# This machine's addresses, as seen from the network the test device is on.
# Under `--network host` the container sees the host's real interfaces.
if command -v ip >/dev/null 2>&1; then
    while read -r addr; do
        [ -n "$addr" ] && ips+=("$addr")
    done < <(ip -4 -o addr show scope global 2>/dev/null | awk '{split($4,a,"/"); print a[1]}')
elif command -v hostname >/dev/null 2>&1; then
    for addr in $(hostname -I 2>/dev/null || true); do
        ips+=("$addr")
    done
fi

for extra in "$@"; do
    # An all-digits-and-dots token is an IPv4 literal; anything with a colon is
    # IPv6; everything else is a DNS name. Misclassifying is not subtle — the
    # browser simply refuses to match — so this is deliberately dumb and total.
    if [[ "$extra" =~ ^[0-9.]+$ ]] || [[ "$extra" == *:* ]]; then
        ips+=("$extra")
    else
        dns+=("$extra")
    fi
done

# Deduplicate while preserving order, then build the SAN list.
san=""
seen=""
add_san() {
    case " $seen " in *" $1 "*) return 0 ;; esac
    seen="$seen $1"
    san="${san:+$san,}$2:$1"
}
for d in "${dns[@]}";  do add_san "$d" DNS; done
for i in "${ips[@]}";  do add_san "$i" IP;  done

# --- the CA: minted once, reused forever -------------------------------------
if [ ! -f "$ca_crt" ] || [ ! -f "$ca_key" ]; then
    echo "dev-cert: minting a new local CA (install this once per test device)"
    openssl req -x509 -newkey rsa:2048 -sha256 -days 730 -nodes \
        -keyout "$ca_key" -out "$ca_crt" \
        -subj "/CN=Entity Browser dev CA/O=Entity Browser (development only)" \
        -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
        -addext "keyUsage=critical,keyCertSign,cRLSign" \
        2>/dev/null
    chmod 600 "$ca_key"
else
    echo "dev-cert: reusing the existing CA at $ca_crt"
fi

# --- the leaf: re-minted whenever the address set changes --------------------
# Recording the SAN set beside the cert is what makes "did my IP change?"
# answerable without parsing the certificate back out. A stale leaf is the
# failure this avoids: you move networks, the cert no longer covers your
# address, and the browser reports a name mismatch that looks like a bad cert.
san_stamp="$out/dev.sans"
if [ -f "$leaf_crt" ] && [ -f "$san_stamp" ] && [ "$(cat "$san_stamp")" = "$san" ]; then
    echo "dev-cert: leaf already covers $san"
else
    echo "dev-cert: minting a leaf for $san"
    openssl req -newkey rsa:2048 -nodes -keyout "$leaf_key" \
        -subj "/CN=entity-browser-dev" -out "$out/dev.csr" 2>/dev/null
    openssl x509 -req -in "$out/dev.csr" -sha256 -days 365 \
        -CA "$ca_crt" -CAkey "$ca_key" -CAcreateserial \
        -extfile <(printf 'subjectAltName=%s\nextendedKeyUsage=serverAuth\n' "$san") \
        -out "$leaf_crt" 2>/dev/null
    rm -f "$out/dev.csr"
    chmod 600 "$leaf_key"
    printf '%s' "$san" > "$san_stamp"
fi

cat <<EOF

  cert  $leaf_crt
  CA    $ca_crt
  names $san

  Install the CA on every device you test from — the leaf alone is not enough,
  and a browser that does not trust the CA will refuse to register a service
  worker even if you click through the warning:

    Linux (Chrome/Chromium, per-user NSS store)
      certutil -d sql:\$HOME/.pki/nssdb -A -t "C,," -n entity-dev-ca -i $ca_crt
    Firefox (any platform)
      Settings -> Privacy & Security -> Certificates -> View Certificates
      -> Authorities -> Import -> tick "Trust this CA to identify websites"
    Android
      copy $ca_crt to the device, then
      Settings -> Security -> Encryption & credentials -> Install a certificate
      -> CA certificate
    iOS
      AirDrop/email the .crt, install the profile, THEN enable it under
      Settings -> General -> About -> Certificate Trust Settings

  Quick alternative, no CA install, Chrome only — treat the plain-HTTP origin as
  secure. Works on an unrooted Android phone via the flags UI, and the origin
  must match scheme, host AND port exactly:

    chrome://flags/#unsafely-treat-insecure-origin-as-secure

EOF

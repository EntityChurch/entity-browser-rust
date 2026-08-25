#!/usr/bin/env bash
# THE MULTI-HOST FEDERATION ORIGIN — the publisher on its own host, so the
# consumer's every fetch is a real network hop instead of a loopback one.
#
#   bash tools/e2e/federation-multihost.sh up|down|probe
#
# WHY THIS EXISTS, precisely. `a_name_resolves_cross_origin_to_a_verified_page_in_a_browser`
# already walks the whole chain in a real browser across two ORIGINS — real HTTP,
# real CORS, real `window.fetch`. What it cannot vary is the HOST: both origins
# are `localhost:<port>`, so publisher and consumer share a network namespace and
# every hop is loopback. `EXTENSION-SIGNALING` §11.5.1 names that blindness class
# — a loopback run reports success for things that cannot work off it, and the
# tell is that "every individual step reports success."
#
# So this moves ONLY the topology. The browser walk is byte-identical; publisher
# and consumer become separate containers with separate routable addresses.
#
#   host                     the cargo test only — it orchestrates, it serves nothing
#     │                                          control :4446 ──┐
#   entity-fed ─┬─ fed-pub 10.89.x.2:8099   the PUBLISHER  (dist-federation)
#               ├─ fed-app 10.89.x.3:8092   the APP origin (dist/)
#               └─ fed-ff  10.89.x.4        selenium/firefox — the CONSUMER
#
# Three distinct addresses, asserted as distinct. `-p` carries ONLY the WebDriver
# control channel; every fetch under test stays on the bridge. The host serves
# nothing on purpose — see the app-container note below for what that cost.
#
# WHAT A GREEN RUN HERE CLAIMS, AND WHAT IT DOES NOT. It claims: a separate
# network namespace, a distinct routable address, a real TCP hop, real
# cross-origin CORS enforced by a real browser, and hash+signature verification
# at a consumer that shares NO process and NO filesystem with the publisher.
# It does NOT claim: two physical machines, the public internet, TLS, a real CDN,
# or any NAT. Those are the next rungs and this rig does not model them — said
# here because a rig that overstates its scope is how a green gate launders an
# untested claim (the NAT rig's `split`-vs-`nat` lesson, one layer up).
#
# THE ORDERING IS LOAD-BEARING. A binding's origin is baked in at PUBLISH time,
# so the publisher's IP must be known BEFORE the emit. That is why the container
# is created first, its address read back, and only then is the federation
# published. Publishing first and hoping to serve it somewhere else yields a
# federation whose bindings point at an origin nothing is listening on — which
# resolves as a withholding origin, not as a misconfigured rig.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
NET=entity-fed
PUB=entity-fed-pub
PORT="${FED_PORT:-8099}"
OUT="${FED_OUT:-dist-federation-multihost}"
IMG="${FED_IMG:-docker.io/library/python:3.12-alpine}"
APP=entity-fed-app
APP_PORT="${APP_PORT:-8092}"
FF=entity-fed-ff
FF_PORT="${FF_PORT:-4446}"
FF_IMG="${FF_IMG:-docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404}"

# Logs go to STDERR so STDOUT is nothing but the consumer's environment —
# the caller evals it, and a stray log line would become a bogus variable.
log() { printf '  %s\n' "$*" >&2; }

down() {
  podman rm -f "$PUB" "$APP" "$FF" >/dev/null 2>&1 || true
  podman network rm -f "$NET" >/dev/null 2>&1 || true
  log "torn down"
}

up() {
  down
  podman network create "$NET" >/dev/null
  # Start the server container FIRST but idle, so its address exists before the
  # publish that must bake that address into every binding.
  podman run -d --name "$PUB" --network "$NET" \
    -v "$REPO:/repo:z" -w /repo "$IMG" \
    sleep infinity >/dev/null

  local ip
  ip="$(podman inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$PUB")"
  [ -n "$ip" ] || { echo "FATAL: no address on $PUB" >&2; exit 1; }

  # THE CONTROL: the address must not be loopback and must not be the host's own.
  # A rootless podman quirk that mirrored the host address into the container
  # would make "multi-host" a lie that every downstream assertion still passes —
  # the same shape as the WebRTC rig's colliding host candidates.
  case "$ip" in
    127.*|localhost) echo "FATAL: publisher address $ip is loopback" >&2; exit 1 ;;
  esac
  if ip -4 addr show 2>/dev/null | grep -qw "$ip"; then
    echo "FATAL: publisher address $ip is also a host address — not a separate host" >&2
    exit 1
  fi

  log "publisher host: $ip:$PORT"

  # Publish with the publisher's REAL origin baked in. Runs on the host (it needs
  # cargo); only the SERVING is in the container, which is what makes the two
  # different hosts.
  ORIGIN_BASE="http://$ip:$PORT/" bash "$REPO/tools/local-federation.sh" "$REPO/$OUT" >/dev/null

  local reg
  reg="$(awk '/registry peer/{getline; gsub(/^[ \t]+|[ \t]+$/,""); print; exit}' "$REPO/$OUT/MAPPING.txt")"
  [ -n "$reg" ] || { echo "FATAL: no registry peer-id in $OUT/MAPPING.txt" >&2; exit 1; }

  # stderr is deliberately NOT discarded — a bind failure is the one error that
  # makes this rig lie (it presents to the browser as a bare NetworkError that
  # reads like an app bug). Same rule as the in-process server.
  # `>/dev/null` on the exec ID, not decoration: STDOUT here IS the consumer's
  # environment, and `podman exec -d` prints the exec ID to it. The first run
  # leaked that hex straight into the caller's `sed 's/^/-e /'` and passed
  # `-e <64-hex>` to the test container — exactly the stray-line failure the
  # `log()` note two screens up predicts, arriving from the one line that was not
  # a log. Anything added here that writes to stdout must be redirected too.
  podman exec -d "$PUB" python3 /repo/tools/cors-serve.py "/repo/$OUT" "$PORT" >/dev/null

  # PROBE FROM THE NETWORK, NOT FROM THE HOST — measured, not assumed.
  #
  # The first version of this probed from the host and failed with the server
  # demonstrably running (`Address in use` inside the container). **Under rootless
  # podman the host has no route into a bridge network's address space** — the
  # same constraint this repo already records for port-publishing ("rootless -p is
  # unreliable under pasta/slirp, so serve uses host-net"), arriving from the
  # other direction.
  #
  # The consequence is a REQUIREMENT on the consumer, not a defect here: **the
  # browser must be a container on this network**, exactly as the WebRTC rigs put
  # both firefox containers on one `podman network create` bridge. A host-network
  # selenium cannot reach this origin at all. So the probe runs from a throwaway
  # container on `$NET` — the path the consumer will actually take.
  local ok=0
  for _ in $(seq 1 40); do
    if podman run --rm --network "$NET" "$IMG" \
         python3 -c "
import sys,urllib.request
try:
    r = urllib.request.urlopen('http://$ip:$PORT/MAPPING.txt', timeout=2)
    sys.exit(0 if r.headers.get('access-control-allow-origin') else 3)
except Exception:
    sys.exit(4)
" >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.5
  done
  [ "$ok" = 1 ] || { echo "FATAL: origin http://$ip:$PORT not healthy or sending no CORS (probed from $NET)" >&2; podman logs "$PUB" >&2 || true; exit 1; }

  log "origin healthy + CORS"

  # THE CONSUMER'S BROWSER, on the same bridge — this is the half that makes it a
  # gate rather than a rig. It must be a container here, because rootless podman
  # gives the host no route inward (see the probe above), so the host-network
  # Selenium every other test uses cannot reach the publisher at all.
  #
  # `-p` publishes ONLY the WebDriver control port. The data path — the browser's
  # fetches at the publisher — stays on the bridge. That split is the WebRTC rigs'
  # pattern (`rung1_repro.sh` publishes 4446/4447 and keeps the media on the
  # network), and it is what lets a host-run cargo drive a browser it cannot
  # otherwise address.
  podman run -d --name "$FF" --network "$NET" -p "$FF_PORT:4444" \
    --shm-size=2g "$FF_IMG" >/dev/null
  local ok=0
  for _ in $(seq 1 60); do
    curl -fsS -m 2 -o /dev/null "http://localhost:$FF_PORT/status" 2>/dev/null && { ok=1; break; }
    sleep 0.5
  done
  [ "$ok" = 1 ] || { echo "FATAL: selenium on :$FF_PORT never came up" >&2; podman logs "$FF" >&2 || true; exit 1; }
  log "browser up, control on :$FF_PORT"

  # THE APP GETS ITS OWN HOST TOO, and this is the second thing rootless podman
  # decided rather than us. The first design served the SPA from the host and
  # pointed the browser at the bridge gateway — Firefox got
  # `connectionFailure ... 10.89.3.1:8092` against a server that was definitely
  # running. **Under rootless podman the bridge and its gateway live inside a
  # netns owned by rootlesskit/pasta, not on the host's interfaces**, so the
  # "gateway" is not the host and the host's listeners are not on it. (The host
  # route reaches containers no better in the other direction — same constraint,
  # seen from the probe above.)
  #
  # Serving the app from its own container is the fix and is also the better
  # shape: three containers, three addresses, and the app origin is now a genuine
  # third party rather than the orchestrator wearing a hat.
  podman run -d --name "$APP" --network "$NET" \
    -v "$REPO:/repo:z" -w /repo "$IMG" \
    python3 /repo/tools/cors-serve.py /repo/dist "$APP_PORT" >/dev/null
  local appip
  appip="$(podman inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$APP")"
  [ -n "$appip" ] || { echo "FATAL: no address on $APP" >&2; exit 1; }
  ok=0
  for _ in $(seq 1 40); do
    if podman run --rm --network "$NET" "$IMG" \
         python3 -c "
import sys,urllib.request
try:
    urllib.request.urlopen('http://$appip:$APP_PORT/index.html', timeout=2); sys.exit(0)
except Exception:
    sys.exit(4)
" >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.5
  done
  [ "$ok" = 1 ] || { echo "FATAL: app origin http://$appip:$APP_PORT not serving — is dist/ built? (\`make wasm\`)" >&2; podman logs "$APP" >&2 || true; exit 1; }
  log "app host: $appip:$APP_PORT"

  # CONTROL: publisher and consumer must not have collapsed onto one address.
  local ffip
  ffip="$(podman inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$FF")"
  # CONTROL: publisher, app and consumer must be three DISTINCT addresses. If any
  # two collapsed, a green run would be claiming a separation it does not have.
  if [ "$ffip" = "$ip" ] || [ "$ffip" = "$appip" ] || [ "$ip" = "$appip" ]; then
    echo "FATAL: addresses collapsed (pub=$ip app=$appip browser=$ffip) — not separate hosts" >&2
    exit 1
  fi
  log "consumer host: $ffip (publisher $ip, app $appip)"

  # Everything the consumer needs, and nothing else. Deliberately printed rather
  # than written to a shared file: the consumer must not learn the pin by reading
  # the publisher's disk, which would be a same-process shortcut wearing a network.
  printf 'E2E_FED_ORIGIN=http://%s:%s\nE2E_FED_REGISTRY=%s\nE2E_WEBDRIVER_URL=http://localhost:%s\nE2E_APP_ORIGIN=http://%s:%s\n' \
    "$ip" "$PORT" "$reg" "$FF_PORT" "$appip" "$APP_PORT"
}

probe() {
  local ip
  ip="$(podman inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$PUB" 2>/dev/null || true)"
  [ -n "$ip" ] || { echo "not up"; exit 1; }
  curl -fsS -D - -o /dev/null "http://$ip:$PORT/MAPPING.txt"
}

case "${1:-up}" in
  up) up ;;
  down) down ;;
  probe) probe ;;
  *) echo "usage: $0 up|down|probe" >&2; exit 2 ;;
esac

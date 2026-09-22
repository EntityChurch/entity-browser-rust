#!/usr/bin/env bash
# TOPOLOGY=nat — two peers behind two SEPARATE NATs, which is the topology the
# split rig cannot build and the only one where a STUN result means anything.
#
#   host          : signaling node (:4071), dist (:8092), STUN responder (:3478)
#                   — those are SLOT 0's ports; RTC_SLOT=n offsets each by n*10
#                     and suffixes every container and network name below.
#     │  (transit is an ordinary podman network, so it reaches the host)
#   transit ──── router-a ──── lan-a (isolate=true) ──── rtc-a
#           └─── router-b ──── lan-b (isolate=true) ──── rtc-b
#
# WHAT MAKES THIS DIFFERENT FROM `split`. Split gives both containers ONE
# external address (host masquerade), so a reflexive candidate learned there is
# the same address for both peers and proves nothing about traversal. Here each
# peer's traffic is masqueraded by its OWN router onto transit, so the two are
# seen as two distinct external addresses — and the STUN responder prints them,
# so that is measured rather than assumed.
#
# WHAT KIND OF NAT THIS IS, precisely, because it bounds what a green run means.
# Linux conntrack gives endpoint-independent MAPPING (one mapping per source
# port, whoever it is talking to — so the reflexive candidate is stable and
# usable) with address/port-dependent FILTERING (an unsolicited inbound packet
# is dropped). That is a **port-restricted cone** NAT: the common home-router
# behaviour, and the case ICE's simultaneous connectivity checks exist to punch
# through — each side's outbound check opens the pinhole for the other's. It is
# NOT symmetric NAT, which changes the mapping per destination and defeats STUN
# entirely; that case needs TURN and this rig does not model it.
#
# The isolate=true LANs are what forbid the direct path: the host will not
# forward between two isolated podman networks, so nothing reaches B except
# through B's external address. Router forwarding is unaffected — it happens
# inside the router's own netns, not across the host's bridges.
#
#   bash nat_topology.sh up|down|probe
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"

# RTC_SLOT — see the long note in rung1_repro.sh. This file builds its own
# containers and networks and MUST land in the same slot: it names `rtc-a` and
# `rtc-b` too, so a `TOPOLOGY=nat` run in one slot would otherwise tear down the
# shared rig's browsers in another. Inherited from rung1_repro.sh when invoked by
# it; derived identically when run standalone, so the two can never disagree.
RTC_SLOT="${RTC_SLOT:-0}"
case "$RTC_SLOT" in
  ''|*[!0-9]*) echo "!! RTC_SLOT must be a non-negative integer (got '$RTC_SLOT')" >&2; exit 1 ;;
esac
[ "$RTC_SLOT" -gt 9 ] && { echo "!! RTC_SLOT must be 0-9 (got $RTC_SLOT)" >&2; exit 1; }
SFX=""
[ "$RTC_SLOT" -ne 0 ] && SFX="-$RTC_SLOT"
STRIDE=$(( RTC_SLOT * 10 ))

TRANSIT=entity-rtc-transit$SFX
LAN_A=entity-rtc-lan-a$SFX
LAN_B=entity-rtc-lan-b$SFX
CTR_A="${CTR_A:-rtc-a$SFX}"
CTR_B="${CTR_B:-rtc-b$SFX}"
CTR_STUN=rtc-stun$SFX
PORT_A=$(( 4446 + STRIDE ))
PORT_B=$(( 4447 + STRIDE ))
STUN_ADDR_FILE=/tmp/entity-rtc-stun-addr$SFX
# The probe below fetches the host-served dist through each NAT, so it needs the
# same port rung1_repro.sh serves on. Derived, never inherited: this file must
# give the same answer run standalone as it does invoked from the rig.
DISTPORT=$(( 8092 + STRIDE ))
# The two IMAGES stay unsuffixed on purpose: they are built from fixed
# Containerfiles, read-only at run time, and no teardown removes them — so
# sharing them across slots is free, where sharing a container is the bug.
ROUTER_IMG=localhost/entity-rtc-router
FF_IMG=localhost/entity-rtc-firefox
# The responder binds inside its own container on this slot's own transit
# network, so strictly it need not move. It moves anyway: one knob offsetting
# every port is what makes "which slot am I in" answerable from a `ss -ltnp`
# line, and a single exception is what makes the next reader check each one.
STUN_PORT="${STUN_PORT:-$(( 3478 + STRIDE ))}"
# How long a UDP mapping lives on the routers with no traffic. 180s is the
# ordinary run's value (generous — nothing should expire mid-establishment).
# The `survives idle` gate LOWERS it, because a quiet window shorter than the
# mapping lifetime tests nothing: the mapping was never at risk and a peer that
# runs no keepalive at all would pass. Real home routers commonly sit at 30-120s
# for UDP, so a low value here is not an unfair substrate, it is the honest one.
UDP_TIMEOUT="${UDP_TIMEOUT:-180}"

down() {
  podman rm -f "$CTR_A" "$CTR_B" "rtc-router-a$SFX" "rtc-router-b$SFX" "$CTR_STUN" >/dev/null 2>&1 || true
  podman network rm -f "$LAN_A" "$LAN_B" "$TRANSIT" >/dev/null 2>&1 || true
  # Scoped to THIS slot's port. Rootless podman leaves a container's processes
  # visible in the host process table, so the bare `pkill -f stun_responder.py`
  # this replaced would reach into every other slot's responder — the same
  # cross-seat demolition the slot mechanism exists to stop, one process over.
  pkill -f "stun_responder.py 0.0.0.0 $STUN_PORT" 2>/dev/null || true
  rm -f "$STUN_ADDR_FILE"
}

build_images() {
  podman image exists "$ROUTER_IMG" || {
    echo ">> building the router image (iptables + iproute2 + python3)"
    podman build -q -t "$ROUTER_IMG" -f "$HERE/Containerfile.router" "$HERE" >/dev/null
  }
  podman image exists "$FF_IMG" || {
    echo ">> building the firefox image (selenium + iproute2)"
    podman build -q -t "$FF_IMG" -f "$HERE/Containerfile.firefox" "$HERE" >/dev/null
  }
}

up() {
  build_images
  echo ">> networks: transit (routable) + two ISOLATED lans"
  podman network exists "$TRANSIT" || podman network create "$TRANSIT" >/dev/null
  podman network exists "$LAN_A" || podman network create --opt isolate=true "$LAN_A" >/dev/null
  podman network exists "$LAN_B" || podman network create --opt isolate=true "$LAN_B" >/dev/null

  # Routers: transit first so eth0 is the outside interface, then the lan.
  # `ip_forward` is namespaced, so the container sets its own without touching
  # the host's.
  for s in a b; do
    lan_var="LAN_${s^^}"
    podman container exists "rtc-router-$s$SFX" || podman run -d --rm \
      --name "rtc-router-$s$SFX" \
      --network "$TRANSIT" --network "${!lan_var}" \
      --cap-add NET_ADMIN \
      --sysctl net.ipv4.ip_forward=1 \
      --sysctl net.netfilter.nf_conntrack_udp_timeout="$UDP_TIMEOUT" \
      --sysctl net.netfilter.nf_conntrack_udp_timeout_stream="$UDP_TIMEOUT" \
      "$ROUTER_IMG" >/dev/null
  done

  # The STUN responder lives ON TRANSIT, and that placement is the measurement.
  # Run on the host instead and rootless podman masquerades container traffic a
  # SECOND time onto the host's own LAN address, so both peers are seen as one
  # address and the rig silently degrades into `split`. Measured, not feared:
  # the first run of this rig reported both peers as 192.168.68.55. On transit
  # the packets stop at the bridge and each peer is seen as its own router.
  podman container exists "$CTR_STUN" || podman run -d --rm --name "$CTR_STUN" \
    --network "$TRANSIT" -v "$HERE/stun_responder.py:/stun.py:z" \
    "$ROUTER_IMG" python3 /stun.py 0.0.0.0 "$STUN_PORT" >/dev/null

  # Browsers: their LAN only. The harness still reaches geckodriver directly
  # from the host (published port on the lan bridge) — that is the control
  # channel, deliberately not the peer path, which is what the routers carry.
  podman container exists "$CTR_A" || podman run -d --rm --name "$CTR_A" \
    --network "$LAN_A" --cap-add NET_ADMIN -p $PORT_A:4444 "$FF_IMG" >/dev/null
  podman container exists "$CTR_B" || podman run -d --rm --name "$CTR_B" \
    --network "$LAN_B" --cap-add NET_ADMIN -p $PORT_B:4444 "$FF_IMG" >/dev/null

  local sub_a sub_b
  sub_a=$(podman network inspect "$LAN_A" --format '{{(index .Subnets 0).Subnet}}')
  sub_b=$(podman network inspect "$LAN_B" --format '{{(index .Subnets 0).Subnet}}')

  for s in a b; do
    lan_var="LAN_${s^^}"; lan="${!lan_var}"
    # Re-apply at exec time, not only at create: `up` reuses a router that is
    # already running, so a run that lowered UDP_TIMEOUT would silently inherit
    # the previous run's value and its idle window would test nothing.
    podman exec "rtc-router-$s$SFX" sysctl -qw \
      net.netfilter.nf_conntrack_udp_timeout="$UDP_TIMEOUT" \
      net.netfilter.nf_conntrack_udp_timeout_stream="$UDP_TIMEOUT" >/dev/null 2>&1 || \
      echo "   ! router-$s: could not set conntrack UDP timeout to ${UDP_TIMEOUT}s"
    r_lan_ip=$(podman inspect "rtc-router-$s$SFX" \
      --format "{{(index .NetworkSettings.Networks \"$lan\").IPAddress}}")
    r_transit_ip=$(podman inspect "rtc-router-$s$SFX" \
      --format "{{(index .NetworkSettings.Networks \"$TRANSIT\").IPAddress}}")
    lan_subnet=$(podman network inspect "$lan" --format '{{(index .Subnets 0).Subnet}}')
    # A private LAN address is not routable from outside it, and here that has
    # to be enforced rather than assumed: podman's `isolate=true` stops the HOST
    # forwarding between the two lans, but each router also holds a default
    # route to the host, so without this it would happily carry rtc-a's packets
    # to rtc-b's private address and the pair would "traverse" over a path no
    # real NAT'd peer has. Measured: the first probe run caught exactly that.
    other_subnet=$([ "$s" = a ] && echo "$sub_b" || echo "$sub_a")
    podman exec "rtc-router-$s$SFX" iptables -C FORWARD -d "$other_subnet" -j DROP 2>/dev/null || \
      podman exec "rtc-router-$s$SFX" iptables -I FORWARD -d "$other_subnet" -j DROP
    # DROP unsolicited inbound UDP, and understand why before touching it: this
    # is the difference between a NAT that can be punched and one that cannot.
    # Without it the router ACCEPTS an unsolicited packet, which CONFIRMS a
    # conntrack entry whose reply tuple is (external_ip:advertised_port -> peer)
    # — exactly the tuple that peer's own outbound punch needs. The outbound
    # then loses the race for its own advertised port and is remapped to a fresh
    # one, so each side sends from a port the other never heard of and the punch
    # can never converge.
    #
    # Measured: with the packets tracked, a bare UDP punch between these two
    # containers reported NO-PACKETS, and conntrack showed A leaving from :32941
    # having advertised :60449. A real NAT drops unsolicited inbound silently
    # and keeps no state for it, which is what this restores — a dropped packet
    # leaves its conntrack entry unconfirmed, so it cannot poison the mapping.
    # The pinhole opened by an OUTBOUND packet still works: that return traffic
    # is ESTABLISHED, not NEW.
    transit_subnet=$(podman network inspect "$TRANSIT" --format '{{(index .Subnets 0).Subnet}}')
    for chain in INPUT FORWARD; do
      podman exec "rtc-router-$s$SFX" iptables -C "$chain" -s "$transit_subnet" -p udp -m conntrack --ctstate NEW -j DROP 2>/dev/null || \
        podman exec "rtc-router-$s$SFX" iptables -I "$chain" -s "$transit_subnet" -p udp -m conntrack --ctstate NEW -j DROP
    done
    # Masquerade by SOURCE SUBNET rather than by output interface: podman names
    # interfaces in flag order, and a rename upstream would silently turn this
    # into a no-op that still looks like a NAT.
    podman exec "rtc-router-$s$SFX" iptables -t nat -C POSTROUTING -s "$lan_subnet" -j MASQUERADE 2>/dev/null || \
      podman exec "rtc-router-$s$SFX" iptables -t nat -A POSTROUTING -s "$lan_subnet" -j MASQUERADE
    echo "   router-$s: lan $r_lan_ip  external $r_transit_ip  (masquerading $lan_subnet, udp mapping ${UDP_TIMEOUT}s)"
    # Point the browser's default route at its router. Everything the peer path
    # needs — node, dist, STUN, the other peer — is beyond it; the LAN subnet
    # itself keeps its on-link route, so the harness still gets its replies.
    podman exec -u root "rtc-$s" ip route replace default via "$r_lan_ip"
  done

  STUN_IP=$(podman inspect "$CTR_STUN" --format "{{(index .NetworkSettings.Networks \"$TRANSIT\").IPAddress}}")
  echo "   stun: stun:$STUN_IP:$STUN_PORT (on transit, so it sees each router)"
  # The rig reads this to build the connector row's `ice=` value.
  echo "$STUN_IP:$STUN_PORT" > "$STUN_ADDR_FILE"
}

# The rig's own control. A topology that silently failed would "prove" that
# host candidates traverse NATs, so every property is probed, not assumed.
probe() {
  local rc=0
  A_IP=$(podman inspect "$CTR_A" --format "{{(index .NetworkSettings.Networks \"$LAN_A\").IPAddress}}")
  B_IP=$(podman inspect "$CTR_B" --format "{{(index .NetworkSettings.Networks \"$LAN_B\").IPAddress}}")
  RA_EXT=$(podman inspect "rtc-router-a$SFX" --format "{{(index .NetworkSettings.Networks \"$TRANSIT\").IPAddress}}")
  RB_EXT=$(podman inspect "rtc-router-b$SFX" --format "{{(index .NetworkSettings.Networks \"$TRANSIT\").IPAddress}}")

  # 1. No direct path. If this leaks, everything below is meaningless.
  local direct
  direct=$(podman exec "$CTR_A" timeout 6 curl -s -m 4 -o /dev/null -w '%{http_code}' \
    "http://$B_IP:4444/status" 2>/dev/null || true)
  if [ "$direct" = "200" ]; then
    echo "   ✗ ISOLATION LEAKED: rtc-a reached rtc-b directly at $B_IP"
    rc=1
  else
    echo "   ✓ no direct A->B path (curl: '${direct:-timeout}')"
  fi

  # 2. The routers give two DISTINCT external addresses — the whole point.
  if [ "$RA_EXT" = "$RB_EXT" ]; then
    echo "   ✗ both peers share one external address ($RA_EXT) — this is `split` again"
    rc=1
  else
    echo "   ✓ two distinct external addresses: A via $RA_EXT, B via $RB_EXT"
  fi

  # 3. Each peer can reach the host THROUGH its router (node + dist + STUN).
  # `host.containers.internal` (a link-local podman injects, NOT the default
  # gateway — the first version of this probe used the gateway and reported a
  # broken NAT that was not broken) is the same name the spike hands the
  # browsers, so this probes the address actually used.
  for s in a b; do
    local code
    code=$(podman exec "rtc-$s$SFX" timeout 8 curl -s -m 6 -o /dev/null -w '%{http_code}' \
      "http://host.containers.internal:$DISTPORT/index.html" 2>/dev/null || true)
    if [ "$code" = "200" ]; then
      echo "   ✓ rtc-$s$SFX reaches the host-served dist through its NAT ($code)"
    else
      echo "   ✗ rtc-$s$SFX could NOT reach the host through its NAT (got '${code:-timeout}')"
      rc=1
    fi
  done
  return $rc
}

case "${1:-up}" in
  up) up ;;
  down) down ;;
  probe) probe ;;
  *) echo "usage: $0 up|down|probe" >&2; exit 2 ;;
esac

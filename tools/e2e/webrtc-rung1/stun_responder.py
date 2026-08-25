#!/usr/bin/env python3
"""A minimal RFC 5389 STUN server — Binding Request in, XOR-MAPPED-ADDRESS out.

WHY OURS AND NOT A PUBLIC ONE. `session_config.rs` refuses to default to a public
reflector because that would enrol a third party invisibly; a rig that then
pointed at `stun.l.google.com` to prove traversal would be testing Google's
availability and the host's internet, and would make a green run depend on
neither of the things under test. This is ~80 lines, runs in the same podman
network as the NAT routers, and makes the measurement self-contained.

WHAT IT MUST GET RIGHT for the result to mean anything:

  - The XOR encoding (§15.2). The mapped port is XORed with the high 16 bits of
    the magic cookie and the address with the whole cookie. A plain MAPPED-ADDRESS
    would be silently rewritten by some NATs that inspect payloads — the exact
    reason XOR-MAPPED-ADDRESS exists — so getting this wrong could make a NAT
    look transparent when it is not.
  - Replying from the SAME socket the request arrived on. The reply's source
    must match the request's destination or conntrack will not match it back
    through the NAT, and the client sees nothing.
  - Echoing the 96-bit transaction id. A client that cannot match the reply to
    its request discards it, which reads exactly like packet loss.

Deliberately NOT implemented: authentication (§9.3 forbids authenticating a
reflector), FINGERPRINT, CHANGE-REQUEST/RFC 5780 behaviour discovery, and TURN.
A binding request is all an ICE agent sends to gather a server-reflexive
candidate, and anything beyond it would be untested code pretending to be
infrastructure.

    python3 stun_responder.py [bind_ip] [port]     # default 0.0.0.0 3478
"""
import socket
import struct
import sys

MAGIC_COOKIE = 0x2112A442
BINDING_REQUEST = 0x0001
BINDING_SUCCESS = 0x0101
ATTR_XOR_MAPPED_ADDRESS = 0x0020
FAMILY_IPV4 = 0x01


def xor_mapped_address(ip: str, port: int) -> bytes:
    """§15.2 XOR-MAPPED-ADDRESS attribute for an IPv4 reflexive transport addr."""
    xport = port ^ (MAGIC_COOKIE >> 16)
    addr = struct.unpack("!I", socket.inet_aton(ip))[0]
    xaddr = addr ^ MAGIC_COOKIE
    value = struct.pack("!BBHI", 0, FAMILY_IPV4, xport, xaddr)
    return struct.pack("!HH", ATTR_XOR_MAPPED_ADDRESS, len(value)) + value


def serve(bind_ip: str, port: int) -> None:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((bind_ip, port))
    print(f"stun: listening on {bind_ip}:{port}", flush=True)
    served = 0
    while True:
        try:
            data, peer = sock.recvfrom(2048)
        except OSError as e:  # noqa: PERF203 - a bad packet must not kill the server
            print(f"stun: recv error {e}", flush=True)
            continue
        if len(data) < 20:
            continue
        msg_type, msg_len, cookie = struct.unpack("!HHI", data[:8])
        txid = data[8:20]
        if cookie != MAGIC_COOKIE or msg_type != BINDING_REQUEST:
            # Not STUN, or not the one thing this serves. Silence is the correct
            # response — an error reply would teach a client the wrong lesson.
            continue
        attr = xor_mapped_address(peer[0], peer[1])
        reply = struct.pack("!HHI", BINDING_SUCCESS, len(attr), MAGIC_COOKIE) + txid + attr
        # Same socket, so the source address matches what the client dialed —
        # otherwise the reply never makes it back through the NAT.
        sock.sendto(reply, peer)
        served += 1
        # The whole point of the rig is observing WHICH external address each
        # peer is seen as; printing every binding makes that visible in the log
        # rather than inferred from a candidate that may never be gathered.
        print(f"stun: binding #{served} from {peer[0]}:{peer[1]}", flush=True)


if __name__ == "__main__":
    serve(sys.argv[1] if len(sys.argv) > 1 else "0.0.0.0",
          int(sys.argv[2]) if len(sys.argv) > 2 else 3478)

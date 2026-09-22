#!/usr/bin/env python3
"""THE OTHER PERSON CLOSED THE TAB — a dead channel must be noticed in seconds.

The positive half of the wake arc. `spike_chat_over_webrtc.py` proves a channel
opens and carries messages; this proves the app finds out when one *dies*.

Two browsers meet over §6.5 WebRTC and chat both ways, so the link is real.
Then B navigates away, which closes B's `RTCPeerConnection`. A must stop saying
"Connected" — and it must do so **fast**.

## STATUS: GREEN — and it was red on purpose for a day first

Landed red on 2026-09-08 tracking an open defect, and **not tuned green**:
`DEMOTE_BUDGET_S` never moved off the property we actually want (12s). 40 and
75 were both tried while it was red and 75 did not pass either. What closed it
was two `core/peer` fixes, not a threshold.

Measured after them: **6/6 PASS at 0.5s** on fresh rigs, covering both roles.
Before: ~half detected at 30.6s and the rest not within 75s.

## The three possible detection times

Bytes cross a `MessagePort`, which has no close event and no error, so a dead
`RTCDataChannel` is invisible until something proves otherwise:

  * **~0.5 s** — the transport EOF. `webrtc_session.rs` posts the zero-length
    sentinel `PortReader` surfaces as EOF, the reader that owns this connection
    ends, and a dispatch fails **immediately** — including one issued *after*
    the end, which is the half that was missing. This is what happens now.
  * **~30 s** — the `DEFAULT_REQUEST_TIMEOUT` in `core/peer`'s `remote.rs`, and
    only when a request happens to be in flight.
  * **~130 s** — the §5 keepalive (`3 x (30 s + 10 s) + 10 s`).

## WHY IT WAS BIMODAL — read this before calling any run of this flaky

A §6.5 link is **one** connection carrying **two** handshake roles. The offerer
rule decides the §7.4.1 *initiator* — who speaks HELLO first — and NOT who dials
(`EXTENSION-SIGNALING` §6.5: *"The offerer governs negotiation direction, never
who dials"*; both ICE agents fire outbound). Peer ids are generated fresh each
run, so **A is the dialer in some runs and the acceptor in others** — and the two roles read that one channel through
completely different code: the dialer through `spawn_reader_loop`
(`reader:` / `terminating reader task`), the acceptor through
`handle_connection` (`remote disconnected (EOF)` / `reentry:`). Both halves had
the same defect and only the dialer's was found first, which presented as a
50/50 flake against a deterministic gate.

That is why the log panel prints **on pass as well as failure**, and why the
role needles are in it. A run that does not say which role it got cannot tell a
bimodal product from a flaky rig.

Both defects were the same rule at two sites: a transport primitive that
**evicts** its own binding disarms the §A1 demotion, because
`demote_peer_on_transport_error` fires only while the failed endpoint is still
bound. Neither site writes liveness; both now merely fail fast and leave the
seam armed.

## Anti-vacuity

"A does not say Connected" is also what an A that never connected says, and what
a broken selector says. So the premise is asserted first and independently:
messages must cross **both ways**, and A's Chat reachability row must read
`Connected` *before* B vanishes. Only then does the clock start.

Asserted on the RENDERED row, not on a log line: the claim is that a person
stops being told they are connected to someone who is gone.

Preconditions are `rung1_repro.sh`'s (rtc-a :4446, rtc-b :4447, dist :8092,
node :4071). Pass the node peer id as argv[1]. MODE=direct — the shipped arm.
"""
import json, os, sys, time, urllib.request

MODE = os.environ.get("MODE", "direct").strip().lower()
if MODE not in ("direct", "worker"):
    print(f"!! MODE must be 'direct' or 'worker', got {MODE!r}"); sys.exit(2)

# Seconds A is allowed to keep claiming "Connected" after B is gone. This is the
# property we want, NOT a description of today's behaviour — see the header.
DEMOTE_BUDGET_S = float(os.environ.get("DEMOTE_BUDGET_S", "12"))
# Reported, never asserted: the two fallbacks below the EOF path, so a run says
# WHICH mechanism noticed rather than only whether one did.
DEMOTE_TARGET_S = float(os.environ.get("DEMOTE_TARGET_S", "12"))

A_BASE, B_BASE = "http://localhost:4446", "http://localhost:4447"
APP = "http://host.containers.internal:8092"
NODE_WS = "ws://host.containers.internal:4071"

def rq(base, method, path, body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)

_PREFS = {"media.peerconnection.ice.obfuscate_host_addresses": False}
if MODE == "worker":
    _PREFS["dom.securecontext.allowlist"] = "host.containers.internal"
    _PREFS["dom.securecontext.whitelist"] = "host.containers.internal"
CAPS = {"capabilities": {"alwaysMatch": {
    "browserName": "firefox",
    "moz:firefoxOptions": {"args": ["-headless"], "prefs": _PREFS},
}}}

BOOT = "const l=document.getElementById('dom-layer');const r=l&&(l.shadowRoot||l);return r&&r.querySelector('button.spawn-btn')?'booted':'no';"

READ_PEER_ID = r"""
const log = window.__entity_browser_log || [];
const B58 = '[1-9A-HJ-NP-Za-km-z]{40,}';
const pats = [
  new RegExp('establisher confirmed on the primary peer = (' + B58 + ')'),
  new RegExp('worker bootstrap: generated and persisted fresh primary peer peer_id = (' + B58 + ')'),
  new RegExp('tree change path = \\/(' + B58 + ')\\/'),
];
for (const re of pats) for (let i=log.length-1;i>=0;i--){const m=JSON.stringify(log[i]).match(re);if(m)return m[1];}
return null;
"""

SPAWN = r"""
const [label]=arguments;
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const b of r.querySelectorAll('button.spawn-btn')){if(b.textContent.trim()===label){b.click();return 'clicked';}}
return 'no-btn:'+label;
"""

BIND = r"""
const [pid]=arguments;
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Chat'){
    const inp=sec.querySelector("[data-field='chat-start-peer']");
    if(!inp)return 'no-bind-input';
    inp.focus();inp.value=pid;
    inp.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));
    return 'bound';
  }
}
return 'no-chat-window';
"""

SEND = r"""
const [text]=arguments;
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Chat'){
    const inp=sec.querySelector("[data-field='chat-compose']");
    if(!inp)return 'no-compose';
    inp.focus();inp.value=text;
    inp.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));
    return 'sent';
  }
}
return 'no-chat-window';
"""

MESSAGES = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Chat'){
    const list=sec.querySelector("[data-field='chat-messages']");
    return list?list.textContent:'(no-list)';
  }
}
return '(no-chat-window)';
"""

# The Chat window's reachability row — "who is in this conversation and whether
# we can actually reach them", one chip per other participant, in the app's one
# connection vocabulary.
#
# This surface rather than Peer Connections' Known-devices table, for two
# reasons. It is where the person who reported this was looking: the complaint
# was "it says they're there and nothing sends". And it needs no registry entry
# — Known devices is written by `reach_keeper`'s remember branch, which requires
# a reach INTENT, and binding a chat by peer id never registers one, so that
# table has no row for B at all (measured: `(no-row-for-peer)`, which is what
# the first cut of this gate correctly failed on).
#
# Returns the row text, or a marker naming which lookup failed — a bare "" is
# indistinguishable from a row that exists and says nothing, and the whole gate
# turns on that distinction.
REACH = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Chat'){
    const row=sec.querySelector("[data-field='chat-reachability']");
    if(!row)return '(no-reach-row)';
    return (row.textContent||'').trim()||'(empty-reach)';
  }
}
return '(no-chat-window)';
"""

def ex(base, sid, script, args=None):
    return rq(base, "POST", f"/session/{sid}/execute/sync",
              {"script": script, "args": args or []})["value"]

def new_session(base, node_peer):
    sid = rq(base, "POST", "/session", CAPS)["value"]["sessionId"]
    rq(base, "POST", f"/session/{sid}/timeouts", {"script": 30000})
    worker = "worker=1&" if MODE == "worker" else ""
    url = (f"{APP}/?{worker}webrtc_node={NODE_WS}&webrtc_node_peer={node_peer}"
           f"&webrtc_enable=1&log=trace")
    rq(base, "POST", f"/session/{sid}/url", {"url": url})
    return sid

def wait_boot(base, sid, label):
    for i in range(60):
        time.sleep(1)
        if ex(base, sid, BOOT) == "booted":
            print(f"  {label} booted ({i+1}s)"); return True
    print(f"  {label} FAILED to boot"); return False

def peer_id(base, sid):
    for _ in range(20):
        v = ex(base, sid, READ_PEER_ID)
        if v: return v
        time.sleep(0.5)
    return None

def deliver(base_from, sid_from, base_to, sid_to, text, label, budget=30):
    ex(base_from, sid_from, SEND, [text])
    for i in range(budget):
        time.sleep(1)
        if text in (ex(base_to, sid_to, MESSAGES) or ""):
            print(f"  {label} delivered at t={i+1}s"); return True
    print(f"  {label} NOT delivered within {budget}s"); return False

def main():
    node_peer = sys.argv[1]
    print(f"MODE={MODE}  budget={DEMOTE_BUDGET_S}s")
    print(f"signaling node: {node_peer}\napp: {APP}  node_ws: {NODE_WS}")
    sa = new_session(A_BASE, node_peer); sb = new_session(B_BASE, node_peer)
    print(f"A={sa}\nB={sb}")
    premise_chat = premise_row = False
    demoted_at = None
    final_status = "(not reached)"
    try:
        if not (wait_boot(A_BASE, sa, "A") and wait_boot(B_BASE, sb, "B")): return 1
        pa, pb = peer_id(A_BASE, sa), peer_id(B_BASE, sb)
        print(f"peer A = {pa}\npeer B = {pb}")
        assert pa and pb and pa != pb, "distinct peer ids required"

        print("\n── 1. the link is real ───────────────────────")
        print("open Chat:", ex(A_BASE, sa, SPAWN, ["+ Chat"]), ex(B_BASE, sb, SPAWN, ["+ Chat"]))
        time.sleep(1.5)
        print("bind A->B:", ex(A_BASE, sa, BIND, [pb]), " bind B->A:", ex(B_BASE, sb, BIND, [pa]))
        for i in range(30):
            time.sleep(1)
            js = "return (window.__entity_browser_log||[]).map(e=>JSON.stringify(e)).join('\\n')"
            oa = "data channel is OPEN" in (ex(A_BASE, sa, js) or "")
            ob = "data channel is OPEN" in (ex(B_BASE, sb, js) or "")
            if oa and ob: print(f"  both channels open at t={i+1}s"); break
        got_b = deliver(A_BASE, sa, B_BASE, sb, "hello from A", "A->B")
        got_a = deliver(B_BASE, sb, A_BASE, sa, "reply from B", "B->A")
        premise_chat = got_b and got_a

        print("\n── 2. A's Chat window says B is Connected ────")
        status = "(not read)"
        for i in range(30):
            time.sleep(1)
            status = ex(A_BASE, sa, REACH) or "(null)"
            if "Connected" in status:
                premise_row = True
                print(f"  A's reachability row reads {status!r} at t={i+1}s"); break
            if i % 5 == 4: print(f"  t={i+1}s A's reachability row: {status!r}")
        if not premise_row:
            print(f"  PREMISE FAILED — A never showed B as Connected (last: {status!r})")

        print("\n── 3. B vanishes ─────────────────────────────")
        # Navigate away rather than killing the session: this is the ordinary
        # "closed the tab" shape, it tears down B's RTCPeerConnection the way a
        # real browser does, and it leaves B's session alive for diagnostics.
        t0 = time.time()
        rq(B_BASE, "POST", f"/session/{sb}/url", {"url": "about:blank"})
        print(f"  B navigated to about:blank at t=0.0s")

        while time.time() - t0 < DEMOTE_BUDGET_S:
            final_status = ex(A_BASE, sa, REACH) or "(null)"
            if "Connected" not in final_status:
                demoted_at = time.time() - t0
                print(f"  A stopped saying Connected at t={demoted_at:.1f}s "
                      f"(row now {final_status!r})")
                break
            time.sleep(0.5)
        else:
            final_status = ex(A_BASE, sa, REACH) or "(null)"
            print(f"  A STILL says {final_status!r} after {DEMOTE_BUDGET_S}s")

        # Which of the chain fired. Ordered from the transport upward, so on a
        # failure the first ZERO is the layer to look at.
        #
        # Printed on PASS too, deliberately: this product is bimodal (see the
        # role needles below), and a panel that only appears on failure gives
        # you nothing to compare a failure against. The cost is one script call.
        if True:
            js = "return (window.__entity_browser_log||[]).map(e=>JSON.stringify(e)).join('\\n')"
            log = ex(A_BASE, sa, js) or ""
            needles = [
                # CONTROL: a kernel `tracing::debug!` that fires on every
                # successful dispatch. If this is 0, the panel below is
                # measuring log capture, not the transport (see AP: a zero from
                # an unvalidated needle is not evidence).
                "internal dispatch: remote completed",
                "reader:",                    # any line from the reader loop
                "transport is over",          # our EOF fired
                "data channel closed",        # the dc close event reached us
                "peer connection is",         # ...or the pc went terminal
                "terminating reader task",    # the EOF ended the reader loop itself
                "reader task terminated",     # ...and an in-flight request saw it
                # WHICH SIDE OF THE HANDSHAKE THIS BROWSER IS ON. A §6.5 link is
                # ONE connection with TWO handshake roles (the offerer becomes the
                # §7.4.1 initiator; it does NOT decide who dials), and peer ids are
                # fresh each run — so A is the dialer in some runs, acceptor in others —
                # freshly-generated peer ids, so it flips run to run. The two
                # sides read the same channel through completely different code:
                # the dialer through `spawn_reader_loop` ("reader:"), the
                # acceptor through `handle_connection` ("remote disconnected").
                # A gate that does not print which one it got cannot tell a
                # bimodal product from a flaky rig.
                "remote disconnected (EOF)",  # the ACCEPT loop ended
                "reentry:",                   # ...and a dispatch rode its endpoint
                "send execute",               # a dispatch failed at the transport
                "remote execute to",          # §10 step 1 reported the failure
                "transport-error",            # the §A1 demotion reason
                "keepalive",
                "suspect",
                "disconnected",
            ]
            print(f"\n  [A] log evidence (verdict: {'noticed' if demoted_at is not None else 'DID NOT NOTICE'}):")
            for n in needles:
                print(f"    {n!r:28} : {log.lower().count(n.lower())}")

        print("\n── the peer-vanishes gate ────────────────────")
        print(f"   premise: chat crossed both ways        : {premise_chat}")
        print(f"   premise: A's Chat said B was Connected : {premise_row}")
        if demoted_at is None:
            print(f"   A noticed B was gone                   : NO (>{DEMOTE_BUDGET_S}s)")
            print( "   → the transport EOF is not reaching the worker; the connection is")
            print( "     failing on the 30s request deadline, or not at all if idle.")
        else:
            verdict = ("on the transport EOF" if demoted_at <= DEMOTE_TARGET_S
                       else "on the 30s request deadline, NOT the transport EOF")
            print(f"   A noticed B was gone                   : {demoted_at:.1f}s ({verdict})")
            if demoted_at > DEMOTE_TARGET_S:
                print(f"   → still above the {DEMOTE_TARGET_S:.0f}s target. The EOF fires but does not end the")
                print( "     reader loop, so nothing fails early. Necessary, not yet sufficient.")
        ok = premise_chat and premise_row and demoted_at is not None
        print(f"\nRESULT: {'PASS ✅ a vanished peer is noticed in seconds, not on the 30s deadline' if ok else 'FAIL ❌'}")
        return 0 if ok else 1
    finally:
        for base, sid in ((A_BASE, sa), (B_BASE, sb)):
            try: rq(base, "DELETE", f"/session/{sid}")
            except Exception: pass

if __name__ == "__main__":
    sys.exit(main())

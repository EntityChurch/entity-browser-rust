#!/usr/bin/env python3
"""Two-browser CHAT over WebRTC — the full-flow live proof.

Two real entity-browser peers on bridge containers rtc-a/rtc-b, a host-run
signaling node, host-served dist. Each opens a Chat window and binds the 1:1
with the other (via the peer-id start input) — which makes ChatDelivery subscribe
to the other over a lazily-established WebRTC data channel. Then A sends a message
and we assert it lands in B's Chat window (and B->A). No transport-specific code
in chat: delivery rides the connection pool, so this is the same flow the memory
transport proves, now over a real RTCDataChannel.

Preconditions (rung1_repro.sh sets these up):
  - rtc-a on :4446, rtc-b on :4447, on shared bridge `entity-rtc-spike`
  - dist on host :8092, signaling node on host :4071
Pass the node peer id as argv[1].

MODE (env, default `direct`): which arm hosts the WebRTC establisher.
  - `direct`  — the shipped Direct/IDB arm (main-thread peer, no worker). NO
    `?worker=1`, and NO `dom.securecontext.*` prefs: the Direct arm reaches
    `RTCPeerConnection` in-thread and needs no OPFS/worker secure context, so a
    green run here proves WebRTC on the DEFAULT deployment (the A-series goal).
  - `worker`  — the opt-in Worker/OPFS arm: `?worker=1` + the secure-context
    allowlist OPFS requires over the non-localhost `host.containers.internal`
    origin. The original rung-1 proof; kept so both arms stay provable.
"""
import json, os, sys, time, urllib.request

MODE = os.environ.get("MODE", "direct").strip().lower()
if MODE not in ("direct", "worker"):
    print(f"!! MODE must be 'direct' or 'worker', got {MODE!r}"); sys.exit(2)

A_BASE, B_BASE = "http://localhost:4446", "http://localhost:4447"
APP = "http://host.containers.internal:8092"
NODE_WS = "ws://host.containers.internal:4071"

def rq(base, method, path, body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)

# Raw host ICE candidates cross-container are needed on BOTH arms (mDNS `.local`
# won't resolve between containers). The secure-context allowlist is a WORKER-arm
# need only (OPFS is secure-context-gated); the Direct arm deliberately omits it.
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

OPEN_CHAT = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const b of r.querySelectorAll('button.spawn-btn')){if(b.textContent.trim()==='+ Chat'){b.click();return 'clicked';}}
return 'no-chat-btn';
"""

# Find the Chat window section, then act on a field within it.
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

# ── the node in force is a listed row, and its one button has to work ────────
#
# THIS RIG IS THE ONLY ONE IN THE TREE THAT PROVISIONS BY URL, which is the
# state where the bug lived: `?webrtc_node=…` installs a working establisher and
# writes NOTHING to the connector registry, so the row Peer Connections lists as
# *in use* is synthesized. `Check` resolved its node out of the registry alone
# and answered "no connector with peer-id …" about the node the same window had
# just labelled in use — reported by the operator off a real two-machine run,
# and invisible to every gate here, because none of them had ever pressed it.
#
# The button is asserted on the RENDERED notice, not on an exit code: the whole
# defect was a correct round trip refused before it started, and only the
# sentence on screen can tell the two apart (AP25).
OPEN_PEERS = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const b of r.querySelectorAll('button.spawn-btn')){
  if(b.textContent.trim()==='+ Peer Connections'){b.click();return 'clicked';}}
return 'no-peers-btn';
"""

CLICK_CHECK = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Peer Connections'){
    for(const b of sec.querySelectorAll('button')){
      if(b.textContent.trim()==='Check'){b.click();return 'clicked';}}
    return 'no-check-button';
  }
}
return 'no-peers-window';
"""

# Leaf nodes only: a container's textContent would sweep up the form's help text
# and match on any word we look for.
CHECK_NOTICE = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Peer Connections'){
    const hits=[];
    for(const p of sec.querySelectorAll('p,div,span')){
      const s=(p.textContent||'').trim();
      if(p.children.length===0&&(s.includes('serves')||s.includes('no connector')||
         s.includes('asking')||s.includes('could not')))hits.push(s);
    }
    return hits.join(' || ');
  }
}
return '(no-peers-window)';
"""

HEADER = r"""
const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;
for(const sec of r.querySelectorAll('section.window')){
  const h=sec.querySelector('header h3');
  if(h&&h.textContent.trim()==='Chat'){
    const hd=sec.querySelector('.chat > div');
    return hd?hd.textContent:'(no-header)';
  }
}
return '(no-chat-window)';
"""

def new_session(base, node_peer):
    sid = rq(base, "POST", "/session", CAPS)["value"]["sessionId"]
    rq(base, "POST", f"/session/{sid}/timeouts", {"script": 30000})
    # `?worker=1` selects the Worker/OPFS arm; its absence is the default
    # Direct/IDB arm — the whole point of MODE=direct.
    worker = "worker=1&" if MODE == "worker" else ""
    url = (f"{APP}/?{worker}webrtc_node={NODE_WS}&webrtc_node_peer={node_peer}"
           f"&webrtc_enable=1&log=trace")
    rq(base, "POST", f"/session/{sid}/url", {"url": url})
    return sid

def ex(base, sid, script, args=None):
    return rq(base, "POST", f"/session/{sid}/execute/sync", {"script": script, "args": args or []})["value"]

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

def grep(base, sid, needles):
    js = "return (window.__entity_browser_log||[]).map(e=>JSON.stringify(e))"
    lines = ex(base, sid, js) or []
    hits = {n: 0 for n in needles}
    for l in lines:
        for n in needles:
            if n.lower() in l.lower(): hits[n] += 1
    return hits, len(lines)

def main():
    node_peer = sys.argv[1]
    print(f"MODE={MODE} ({'Direct/IDB — default arm' if MODE=='direct' else 'Worker/OPFS — opt-in arm'})")
    print(f"signaling node: {node_peer}\napp: {APP}  node_ws: {NODE_WS}")
    sa = new_session(A_BASE, node_peer); sb = new_session(B_BASE, node_peer)
    print(f"A={sa}\nB={sb}")
    try:
        if not (wait_boot(A_BASE, sa, "A") and wait_boot(B_BASE, sb, "B")): return 1
        pa, pb = peer_id(A_BASE, sa), peer_id(B_BASE, sb)
        print(f"peer A = {pa}\npeer B = {pb}")
        assert pa and pb and pa != pb, "distinct peer ids required"

        print("open Chat:", ex(A_BASE, sa, OPEN_CHAT), ex(B_BASE, sb, OPEN_CHAT))
        time.sleep(1.5)

        # Bind BOTH directions back-to-back — WebRTC glare needs symmetric drive,
        # and each side must subscribe to the other for a two-way conversation.
        print(f"bind A->B:", ex(A_BASE, sa, BIND, [pb]), " bind B->A:", ex(B_BASE, sb, BIND, [pa]))
        print("  A header:", ex(A_BASE, sa, HEADER))
        print("  B header:", ex(B_BASE, sb, HEADER))

        # Give the WebRTC channel time to establish (subscribe triggers it).
        for i in range(30):
            time.sleep(1)
            ha, _ = grep(A_BASE, sa, ["data channel is OPEN", "live path established"])
            hb, _ = grep(B_BASE, sb, ["data channel is OPEN", "live path established"])
            oa = any(v for v in ha.values()); ob = any(v for v in hb.values())
            if i % 5 == 4:
                print(f"  t={i+1}s channel open: A={oa} B={ob}")
            if oa and ob: print(f"  both channels open at t={i+1}s"); break

        # A sends -> expect it in B's window.
        print("\nA sends:", ex(A_BASE, sa, SEND, ["hello from A over webrtc"]))
        got_b = False
        for i in range(30):
            time.sleep(1)
            msgs = ex(B_BASE, sb, MESSAGES)
            if "hello from A over webrtc" in (msgs or ""):
                got_b = True; print(f"  B received A's message at t={i+1}s"); break
            if i % 5 == 4: print(f"  t={i+1}s B messages: {msgs!r}")

        # B replies -> expect it in A's window.
        print("\nB sends:", ex(B_BASE, sb, SEND, ["reply from B over webrtc"]))
        got_a = False
        for i in range(30):
            time.sleep(1)
            msgs = ex(A_BASE, sa, MESSAGES)
            if "reply from B over webrtc" in (msgs or ""):
                got_a = True; print(f"  A received B's reply at t={i+1}s"); break
            if i % 5 == 4: print(f"  t={i+1}s A messages: {msgs!r}")

        # Diagnostics on failure.
        if not (got_b and got_a):
            for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
                h, n = grep(base, sid, ["chat delivery", "subscribe_at", "data channel is OPEN",
                                        "no live path", "VerificationUnavailable", "Timeout",
                                        "no originating authority", "system/tree", "app/chat"])
                print(f"\n[{lbl}] diag ({n} log lines):", {k: v for k, v in h.items() if v})
            print("\n  final A messages:", repr(ex(A_BASE, sa, MESSAGES)))
            print("  final B messages:", repr(ex(B_BASE, sb, MESSAGES)))

        # ── the node in force answers its own Check ──────────────────────────
        print("\nopen Peer Connections on A:", ex(A_BASE, sa, OPEN_PEERS))
        time.sleep(2)
        print("  click Check:", ex(A_BASE, sa, CLICK_CHECK))
        notice = ""
        for i in range(25):
            time.sleep(1)
            notice = ex(A_BASE, sa, CHECK_NOTICE) or ""
            # "asking …" is the in-flight line; wait for what it settles to.
            if notice and "asking" not in notice:
                break
        print(f"  notice after {i+1}s: {notice!r}")
        # Both halves: it must NOT be the refusal, and it must be the answer.
        # Asserting only the absence would pass on a button that does nothing.
        checked = "serves" in notice and "no connector" not in notice

        print("\n── chat-over-webrtc gate ─────────────────────")
        print(f"   A->B delivered: {got_b}   B->A delivered: {got_a}")
        print(f"   Check answered for the node in force: {checked}")
        ok = got_b and got_a and checked
        print(f"\nRESULT: {'PASS ✅ bidirectional chat over one WebRTC channel' if ok else 'FAIL ❌'}")
        return 0 if ok else 1
    finally:
        rq(A_BASE, "DELETE", f"/session/{sa}"); rq(B_BASE, "DELETE", f"/session/{sb}")

if __name__ == "__main__":
    sys.exit(main())

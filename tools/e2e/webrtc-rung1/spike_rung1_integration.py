#!/usr/bin/env python3
"""rung-1 INTEGRATION spike (Python/WebDriver, real dist) — before codifying to Rust.

Two real entity browser peers (bridge containers rtc-a/rtc-b), a host-run
signaling node, host-served dist. Provision both with webrtc, drive a concurrent
bidirectional cross-peer execute, and watch for a real RTCDataChannel + payload.

Preconditions (already set up this session):
  - rtc-a on :4446, rtc-b on :4447, both on shared bridge `entity-rtc-spike`
  - dist served on host :8092  (browsers reach http://host.containers.internal:8092)
  - signaling node on host :4071 (browsers reach ws://host.containers.internal:4071)
Pass the node peer id as argv[1].
"""
import json, re, sys, time, urllib.request

A_BASE, B_BASE = "http://localhost:4446", "http://localhost:4447"
APP = "http://host.containers.internal:8092"
NODE_WS = "ws://host.containers.internal:4071"

def rq(base, method, path, body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)

CAPS = {"capabilities": {"alwaysMatch": {
    "browserName": "firefox",
    "moz:firefoxOptions": {
        "args": ["-headless"],
        "prefs": {
            "media.peerconnection.ice.obfuscate_host_addresses": False,
            # secure-context allowlist so OPFS works -> Worker mode boots ->
            # webrtc establisher (worker-only) is actually wired.
            "dom.securecontext.allowlist": "host.containers.internal",
            "dom.securecontext.whitelist": "host.containers.internal",
        },
    },
}}}

BOOT_PROBE = r"""
const layer = document.getElementById('dom-layer');
const root = layer && (layer.shadowRoot || layer);
return root && root.querySelector('button.spawn-btn') ? 'booted' : 'not-yet';
"""

# The webrtc-enabled primary peer id is stated verbatim in the confirm line
# `establisher confirmed on the primary peer = <ID>`. Fall back to the worker
# bootstrap line, then any tree-path leading segment.
READ_PEER_ID = r"""
const log = window.__entity_browser_log || [];
const B58 = '[1-9A-HJ-NP-Za-km-z]{40,}';
const pats = [
  new RegExp('establisher confirmed on the primary peer = (' + B58 + ')'),
  new RegExp('worker bootstrap: generated and persisted fresh primary peer peer_id = (' + B58 + ')'),
  new RegExp('tree change path = \\/(' + B58 + ')\\/'),
];
for (const re of pats) {
  for (let i = log.length - 1; i >= 0; i--) {
    const m = JSON.stringify(log[i]).match(re);
    if (m) return m[1];
  }
}
return null;
"""

def new_session(base, node_peer):
    sid = rq(base, "POST", "/session", CAPS)["value"]["sessionId"]
    rq(base, "POST", f"/session/{sid}/timeouts", {"script": 30000})
    url = (f"{APP}/?worker=1&webrtc_node={NODE_WS}&webrtc_node_peer={node_peer}"
           f"&webrtc_enable=1&log=trace")
    rq(base, "POST", f"/session/{sid}/url", {"url": url})
    return sid

def wait_boot(base, sid, label):
    for i in range(60):
        time.sleep(1)
        if rq(base, "POST", f"/session/{sid}/execute/sync", {"script": BOOT_PROBE, "args": []})["value"] == "booted":
            print(f"  {label} booted ({i+1}s)")
            return True
    print(f"  {label} FAILED to boot")
    return False

def peer_id(base, sid):
    # the confirm line lands shortly after the spawn button appears; retry.
    for _ in range(20):
        v = rq(base, "POST", f"/session/{sid}/execute/sync", {"script": READ_PEER_ID, "args": []})["value"]
        if v:
            return v
        time.sleep(0.5)
    return None

OPEN_SHELL = r"""
const layer = document.getElementById('dom-layer');
const root = layer.shadowRoot || layer;
const btns = root.querySelectorAll('button.spawn-btn');
for (const b of btns) { if (b.textContent.trim() === '+ Shell') { b.click(); return 'clicked'; } }
return 'no-shell-btn';
"""

SUBMIT = r"""
const [line] = arguments;
const layer = document.getElementById('dom-layer');
const root = layer.shadowRoot || layer;
for (const sec of root.querySelectorAll('section.window')) {
  const t = sec.querySelector('header h3');
  if (t && t.textContent.trim() === 'Shell') {
    const input = sec.querySelector("[data-field='shell-input']");
    if (!input) return 'no-input';
    input.value = line;
    input.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', code:'Enter', bubbles:true, cancelable:true}));
    return 'submitted';
  }
}
return 'no-shell';
"""

SCROLLBACK = r"""
const layer = document.getElementById('dom-layer');
const root = layer.shadowRoot || layer;
for (const sec of root.querySelectorAll('section.window')) {
  const t = sec.querySelector('header h3');
  if (t && t.textContent.trim() === 'Shell') {
    const pre = sec.querySelector("[data-field='shell-scrollback']");
    return pre ? pre.textContent : '';
  }
}
return '';
"""

def open_shell(base, sid):
    return rq(base, "POST", f"/session/{sid}/execute/sync", {"script": OPEN_SHELL, "args": []})["value"]

def submit(base, sid, line):
    return rq(base, "POST", f"/session/{sid}/execute/sync", {"script": SUBMIT, "args": [line]})["value"]

def scrollback(base, sid):
    return rq(base, "POST", f"/session/{sid}/execute/sync", {"script": SCROLLBACK, "args": []})["value"]

def grep_log(base, sid, needles):
    js = "return (window.__entity_browser_log||[]).map(e=>JSON.stringify(e))"
    lines = rq(base, "POST", f"/session/{sid}/execute/sync", {"script": js, "args": []})["value"] or []
    hits = {n: [] for n in needles}
    for l in lines:
        for n in needles:
            if n.lower() in l.lower():
                hits[n].append(l[:300])
    return hits, len(lines)

def main():
    node_peer = sys.argv[1]
    print(f"signaling node peer: {node_peer}\napp: {APP}  node_ws: {NODE_WS}")
    sa = new_session(A_BASE, node_peer)
    sb = new_session(B_BASE, node_peer)
    print(f"A={sa}\nB={sb}")
    try:
        if not (wait_boot(A_BASE, sa, "A") and wait_boot(B_BASE, sb, "B")):
            return 1
        pa, pb = peer_id(A_BASE, sa), peer_id(B_BASE, sb)
        print(f"peer A = {pa}\npeer B = {pb}")
        assert pa and pb and pa != pb, "peer ids must be distinct + present"

        # --- webrtc provisioning sanity: did each browser install the establisher? ---
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            hits, n = grep_log(base, sid, ["webrtc", "establisher", "signaling"])
            print(f"\n[{lbl}] provisioning-related log ({n} lines total):")
            for k, v in hits.items():
                print(f"  {k}: {len(v)} hits")
                for line in v[:4]:
                    print("     ", line)

        # --- open a Shell in each browser (bound to its own primary peer) ---
        print("\nopening shells:", open_shell(A_BASE, sa), open_shell(B_BASE, sb))
        time.sleep(1.0)

        op = sys.argv[2] if len(sys.argv) > 2 else "list"
        cmd_a = f"exec entity://{pb}/system/tree {op}"
        cmd_b = f"exec entity://{pa}/system/tree {op}"

        # --- SYMMETRIC concurrent drive: both must exec toward each other or it
        #     deadlocks (glare role by peer-id order). Submit back-to-back so both
        #     establish_live() calls are in flight together. ---
        print(f"\nA> {cmd_a}\nB> {cmd_b}")
        print("submit A:", submit(A_BASE, sa, cmd_a), " submit B:", submit(B_BASE, sb, cmd_b))

        # --- poll both scrollbacks; print progress so we see pending vs error ---
        window_s = float(sys.argv[3]) if len(sys.argv) > 3 else 45.0
        settle = lambda t: any(k in t.lower() for k in ["error", "fail", "timed out", "timeout", "revision", "children", '"path"', "entries"])
        a_tail = b_tail = ""
        for i in range(int(window_s * 2)):
            time.sleep(0.5)
            sca, scb = scrollback(A_BASE, sa), scrollback(B_BASE, sb)
            a_tail = sca.split(cmd_a)[-1] if cmd_a in sca else sca
            b_tail = scb.split(cmd_b)[-1] if cmd_b in scb else scb
            if i % 6 == 5:
                print(f"  t={(i+1)*0.5:.0f}s  A:{'settled' if settle(a_tail) else 'pending':7} B:{'settled' if settle(b_tail) else 'pending'}")
            if settle(a_tail) and settle(b_tail):
                print(f"  both settled at t={(i+1)*0.5:.0f}s"); break
        print("\n===== A scrollback tail =====\n", a_tail[:1500])
        print("\n===== B scrollback tail =====\n", b_tail[:1500])
        ok = ("no transport profile" not in a_tail and "no transport profile" not in b_tail
              and (settle(a_tail) or settle(b_tail)))
        print("\nRESULT:", "PASS ✅ real cross-peer round-trip" if ok else "FAIL ❌ (see tails + signaling log)")
        return 0 if ok else 1
    finally:
        rq(A_BASE, "DELETE", f"/session/{sa}")
        rq(B_BASE, "DELETE", f"/session/{sb}")

if __name__ == "__main__":
    sys.exit(main())

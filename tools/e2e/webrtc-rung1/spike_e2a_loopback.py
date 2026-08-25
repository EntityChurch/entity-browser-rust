#!/usr/bin/env python3
"""E2a spike: in-page loopback WebRTC on the existing :4444 Selenium Firefox.

Two RTCPeerConnections in ONE page, iceServers:[] (host candidates only),
manual in-page signaling. Assert a DataChannel opens and a message crosses.
If this fails, headless Firefox WebRTC is broken in this env.
No entity stack, no Rust — pure WebDriver over HTTP.
"""
import json, sys, urllib.request

BASE = "http://localhost:4444"

def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)

LOOPBACK_JS = r"""
const done = arguments[arguments.length - 1];
(async () => {
  try {
    const pc1 = new RTCPeerConnection({iceServers: []});
    const pc2 = new RTCPeerConnection({iceServers: []});
    let c1 = 0, c2 = 0, hostOnly = true;
    pc1.onicecandidate = e => { if (e.candidate) { c1++; if (e.candidate.candidate && !/ host /.test(e.candidate.candidate)) hostOnly = false; pc2.addIceCandidate(e.candidate).catch(()=>{}); } };
    pc2.onicecandidate = e => { if (e.candidate) { c2++; if (e.candidate.candidate && !/ host /.test(e.candidate.candidate)) hostOnly = false; pc1.addIceCandidate(e.candidate).catch(()=>{}); } };
    const dc1 = pc1.createDataChannel("spike");
    const gotMsg = new Promise((resolve) => {
      dc1.onopen = () => { dc1.send("ping-from-1"); };
      pc2.ondatachannel = (ev) => { ev.channel.onmessage = (m) => resolve(m.data); };
    });
    const offer = await pc1.createOffer();
    await pc1.setLocalDescription(offer);
    await pc2.setRemoteDescription(offer);
    const answer = await pc2.createAnswer();
    await pc2.setLocalDescription(answer);
    await pc1.setRemoteDescription(answer);
    const msg = await Promise.race([
      gotMsg,
      new Promise(res => setTimeout(() => res(null), 10000)),
    ]);
    done(JSON.stringify({
      opened: msg !== null, msg,
      pc1Ice: pc1.iceConnectionState, pc2Ice: pc2.iceConnectionState,
      dc1: dc1.readyState, cands1: c1, cands2: c2, hostOnly,
    }));
  } catch (e) {
    done(JSON.stringify({error: String(e), stack: (e && e.stack) || null}));
  }
})();
"""

def main():
    caps = {"capabilities": {"alwaysMatch": {
        "browserName": "firefox",
        "moz:firefoxOptions": {"args": ["-headless"]},
    }}}
    sid = rq("POST", "/session", caps)["value"]["sessionId"]
    print(f"session: {sid}")
    try:
        rq("POST", f"/session/{sid}/timeouts", {"script": 30000})
        rq("POST", f"/session/{sid}/url", {"url": "about:blank"})
        out = rq("POST", f"/session/{sid}/execute/async",
                 {"script": LOOPBACK_JS, "args": []})
        res = json.loads(out["value"])
        print(json.dumps(res, indent=2))
        ok = res.get("opened") is True and res.get("msg") == "ping-from-1"
        print("\nRESULT:", "PASS ✅" if ok else "FAIL ❌")
        return 0 if ok else 1
    finally:
        rq("DELETE", f"/session/{sid}")

if __name__ == "__main__":
    sys.exit(main())

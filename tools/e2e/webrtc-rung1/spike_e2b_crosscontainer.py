#!/usr/bin/env python3
"""E2b spike: cross-container host-candidate WebRTC (host-net A <-> bridge B).

Two SEPARATE Firefox containers (:4444 host-net, :4445 bridge). One
RTCPeerConnection each, non-trickle full-SDP exchange shuttled by this harness,
iceServers:[] (host candidates only), mDNS obfuscation OFF (raw-IP host cands).
Assert a DataChannel opens and a bidirectional message round-trips.
Proves the rung-1 substrate: cross-browser-context WebRTC on one host.
"""
import json, os, re, sys, time, urllib.request

def rq(base, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)

# mDNS obfuscation OFF -> Firefox emits raw-IP host candidates (needed so a
# separate container can route to them; a `.local` mDNS name won't resolve
# across the container boundary).
CAPS = {"capabilities": {"alwaysMatch": {
    "browserName": "firefox",
    "moz:firefoxOptions": {
        "args": ["-headless"],
        "prefs": {"media.peerconnection.ice.obfuscate_host_addresses": False},
    },
}}}

WAIT_GATHER = r"""
function waitGather(pc){return new Promise(res=>{
  if(pc.iceGatheringState==='complete')return res();
  const chk=()=>{if(pc.iceGatheringState==='complete'){pc.removeEventListener('icegatheringstatechange',chk);res();}};
  pc.addEventListener('icegatheringstatechange',chk);
  setTimeout(res,6000);
});}
"""

OFFER_JS = r"""
const done = arguments[arguments.length-1];
(async()=>{ try{
  window.__pc = new RTCPeerConnection({iceServers:[]});
  window.__opened=false; window.__recv=null;
  window.__dc = window.__pc.createDataChannel("spike");
  window.__dc.onopen=()=>{window.__opened=true; window.__dc.send("hello-from-A");};
  window.__dc.onmessage=(m)=>{window.__recv=m.data;};
  const off = await window.__pc.createOffer();
  await window.__pc.setLocalDescription(off);
  await waitGather(window.__pc);
  done(JSON.stringify(window.__pc.localDescription));
}catch(e){done(JSON.stringify({error:String(e), stack:(e&&e.stack)||null}));} })();
""" + WAIT_GATHER

ANSWER_JS = r"""
const offerSdp = arguments[0];
const done = arguments[arguments.length-1];
(async()=>{ try{
  window.__pc = new RTCPeerConnection({iceServers:[]});
  window.__opened=false; window.__recv=null;
  window.__pc.ondatachannel=(ev)=>{ window.__dc=ev.channel;
    window.__dc.onopen=()=>{window.__opened=true;};
    window.__dc.onmessage=(m)=>{window.__recv=m.data; try{window.__dc.send("ack-from-B");}catch(e){}};
  };
  await window.__pc.setRemoteDescription(offerSdp);
  const ans = await window.__pc.createAnswer();
  await window.__pc.setLocalDescription(ans);
  await waitGather(window.__pc);
  done(JSON.stringify(window.__pc.localDescription));
}catch(e){done(JSON.stringify({error:String(e), stack:(e&&e.stack)||null}));} })();
""" + WAIT_GATHER

FINALIZE_JS = r"""
const answerSdp = arguments[0];
const done = arguments[arguments.length-1];
(async()=>{ try{ await window.__pc.setRemoteDescription(answerSdp); done("ok"); }
catch(e){done(JSON.stringify({error:String(e)}));} })();
"""

POLL_JS = "return JSON.stringify({opened:window.__opened, recv:window.__recv, ice:window.__pc.iceConnectionState});"

def cands(sdp):
    return [l.strip() for l in sdp.splitlines() if l.startswith("a=candidate")]

def main():
    # shared-bridge pair; slot-aware (see spike_meet_then_chat.py)
    A = os.environ.get("RTC_A_BASE", "http://localhost:4446")
    B = os.environ.get("RTC_B_BASE", "http://localhost:4447")
    sa = rq(A, "POST", "/session", CAPS)["value"]["sessionId"]
    sb = rq(B, "POST", "/session", CAPS)["value"]["sessionId"]
    print(f"A(host-net :4444)={sa}\nB(bridge :4445)={sb}")
    try:
        for base, sid in ((A, sa), (B, sb)):
            rq(base, "POST", f"/session/{sid}/timeouts", {"script": 30000})
            rq(base, "POST", f"/session/{sid}/url", {"url": "about:blank"})

        offer = json.loads(rq(A, "POST", f"/session/{sa}/execute/async", {"script": OFFER_JS, "args": []})["value"])
        if "error" in offer:
            print("OFFER ERROR:", offer); return 1
        print("\nA host candidates:"); [print("  ", c) for c in cands(offer["sdp"])]

        answer = json.loads(rq(B, "POST", f"/session/{sb}/execute/async", {"script": ANSWER_JS, "args": [offer]})["value"])
        if "error" in answer:
            print("ANSWER ERROR:", answer); return 1
        print("\nB host candidates:"); [print("  ", c) for c in cands(answer["sdp"])]

        fin = rq(A, "POST", f"/session/{sa}/execute/async", {"script": FINALIZE_JS, "args": [answer]})["value"]
        if fin != "ok":
            print("FINALIZE ERROR:", fin); return 1

        print("\nnegotiating (polling up to 15s)...")
        a_state = b_state = None
        for _ in range(30):
            time.sleep(0.5)
            a_state = json.loads(rq(A, "POST", f"/session/{sa}/execute/sync", {"script": POLL_JS, "args": []})["value"])
            b_state = json.loads(rq(B, "POST", f"/session/{sb}/execute/sync", {"script": POLL_JS, "args": []})["value"])
            if a_state["recv"] == "ack-from-B" and b_state["recv"] == "hello-from-A":
                break
        print("A:", a_state)
        print("B:", b_state)
        ok = a_state["recv"] == "ack-from-B" and b_state["recv"] == "hello-from-A"
        print("\nRESULT:", "PASS ✅ cross-container DataChannel + bidirectional msg" if ok else "FAIL ❌")
        return 0 if ok else 1
    finally:
        rq(A, "DELETE", f"/session/{sa}")
        rq(B, "DELETE", f"/session/{sb}")

if __name__ == "__main__":
    sys.exit(main())

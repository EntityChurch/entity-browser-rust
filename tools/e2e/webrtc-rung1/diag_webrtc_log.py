#!/usr/bin/env python3
"""Boot one browser (webrtc_enable=1) and dump every webrtc-related log line."""
import json, sys, time, urllib.request

BASE = "http://localhost:4446"
APP = "http://host.containers.internal:8092"
NODE_WS = "ws://host.containers.internal:4071"

def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)

CAPS = {"capabilities": {"alwaysMatch": {"browserName": "firefox",
    "moz:firefoxOptions": {"args": ["-headless"],
        "prefs": {
            "media.peerconnection.ice.obfuscate_host_addresses": False,
            # Mark host.containers.internal a secure context so OPFS
            # (navigator.storage.getDirectory) is available -> Worker mode boots.
            "dom.securecontext.allowlist": "host.containers.internal",
            "dom.securecontext.whitelist": "host.containers.internal",
        }}}}}

def main():
    node_peer = sys.argv[1]
    sid = rq("POST", "/session", CAPS)["value"]["sessionId"]
    try:
        rq("POST", f"/session/{sid}/timeouts", {"script": 30000})
        url = f"{APP}/?worker=1&webrtc_node={NODE_WS}&webrtc_node_peer={node_peer}&webrtc_enable=1&log=trace"
        rq("POST", f"/session/{sid}/url", {"url": url})
        # wait for boot
        for _ in range(60):
            time.sleep(1)
            b = rq("POST", f"/session/{sid}/execute/sync", {"script":
                "const l=document.getElementById('dom-layer');const r=l&&(l.shadowRoot||l);return r&&r.querySelector('button.spawn-btn')?'y':'n'", "args": []})["value"]
            if b == "y":
                break
        time.sleep(2)  # let install-report round-trip
        lines = rq("POST", f"/session/{sid}/execute/sync",
                   {"script": "return (window.__entity_browser_log||[]).map(e=>JSON.stringify(e))", "args": []})["value"] or []
        print(f"total log lines: {len(lines)}")
        keys = ["webrtc", "establish", "confirm", "shortfall", "primary", "seam",
                "install", "webrtc_enable", "6.5", "§6.5", "signaling", "live"]
        print("=== matching lines ===")
        seen = 0
        for l in lines:
            low = l.lower()
            if any(k.lower() in low for k in keys):
                # strip the console color noise
                try:
                    o = json.loads(l)
                    args = o.get("args", [])
                    txt = args[0] if args else l
                except Exception:
                    txt = l
                print(" •", txt[:400].replace("%c", ""))
                seen += 1
        if not seen:
            print("(no webrtc-related lines found)")
    finally:
        rq("DELETE", f"/session/{sid}")

if __name__ == "__main__":
    sys.exit(main())

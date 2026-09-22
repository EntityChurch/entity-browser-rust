"""Tiny W3C WebDriver client (same approach as tools/run-env/probes/alpine-browser-probe.py: raw JSON over urllib)."""
import base64, json, os, pathlib, time, urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4490")
SHOTS = pathlib.Path(__file__).parent / "shots"


def rq(method, path, body=None, timeout=300):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")


class Session:
    def __init__(self):
        st = rq("GET", "/status")
        for n in st["value"]["nodes"]:
            for s in n["slots"]:
                if s.get("session"):
                    try:
                        rq("DELETE", f"/session/{s['session']['sessionId']}")
                    except Exception:
                        pass
        self.sid = rq("POST", "/session", {"capabilities": {"alwaysMatch": {"browserName": "firefox"}}})["value"]["sessionId"]
        rq("POST", f"/session/{self.sid}/window/rect", {"width": 1280, "height": 1024})

    def go(self, url):
        rq("POST", f"/session/{self.sid}/url", {"url": url})

    def js(self, script, *args):
        return rq("POST", f"/session/{self.sid}/execute/sync", {"script": script, "args": list(args)})["value"]

    def ajs(self, script, *args):
        # async script: last arg is the callback
        rq("POST", f"/session/{self.sid}/timeouts", {"script": 280000})
        return rq("POST", f"/session/{self.sid}/execute/async", {"script": script, "args": list(args)})["value"]

    def shot(self, name):
        SHOTS.mkdir(exist_ok=True)
        p = SHOTS / f"{name}.png"
        p.write_bytes(base64.b64decode(rq("GET", f"/session/{self.sid}/screenshot")["value"]))
        return str(p)

    def actions(self, acts):
        rq("POST", f"/session/{self.sid}/actions", {"actions": acts})

    def keys(self, text):
        self.actions([{"type": "key", "id": "kb", "actions":
                       [a for ch in text for a in ({"type": "keyDown", "value": ch}, {"type": "pause", "duration": 60}, {"type": "keyUp", "value": ch}, {"type": "pause", "duration": 60})]}])

    def close(self):
        try:
            rq("DELETE", f"/session/{self.sid}")
        except Exception:
            pass


def settle(s, budget, quiet_s=4.0, poll=0.5, log=True):
    """Poll the screen hash; return (t_first_gfx_ms, t_settled_s, last) once unchanged for quiet_s."""
    t0 = time.time(); last_h = None; last_change = t0; out = None
    while time.time() - t0 < budget:
        time.sleep(poll)
        out = s.js("return {h: window.__screenHash(), S: {events: __spike.events, sizes: __spike.sizes, mouse: __spike.mouseEnabled, err: __spike.error}}")
        h = out["h"]["hash"]
        if h != last_h:
            last_h = h; last_change = time.time()
        elif time.time() - last_change >= quiet_s:
            return time.time() - t0, last_change - t0, out
    return None, last_change - t0, out

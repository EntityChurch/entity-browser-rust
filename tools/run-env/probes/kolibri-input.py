"""kolibri-input.py -- driving KolibriOS the way a person does, through REAL input.

Shared by kolibri-probe.py (standalone) and apps-window-kolibri-probe.py (in the
player frame). Every event here is a WebDriver action -- the browser's own pointer
and keyboard -- never emulator.bus.send or keyboard_send_text. Those API calls go
around the two things that were broken in the field (2026-09-14): the frame never
took focus, so keys went to the host; and the guest pointer drifted away from the
host pointer. A gate that drives the machine through the API passes with both.

Coordinates are GUEST pixels (the 1024x768 desktop); the helpers turn them into
offsets from the canvas's centre in CSS pixels, so the page's scaling is part of
what is measured, not something the probe works around.
"""
import time

GUEST_W, GUEST_H = 1024, 768
KEY_MS = 150          # KolibriOS drops keys that arrive faster (kolibri-probe.py measured 60 ms losing some)


class Input:
    def __init__(self, rq, sid, js):
        self.rq, self.sid, self.js = rq, sid, js

    def canvas(self):
        v = self.rq("POST", f"/session/{self.sid}/element", {"using": "css selector", "value": "#screen_container canvas"})["value"]
        # This grid answers with a key spelled differently from the W3C constant; echo it back as given.
        key = next(k for k in v if k.startswith("element-"))
        return {key: v[key]}

    def _offset(self, gx, gy):
        r = self.js("const r = document.querySelector('#screen_container canvas').getBoundingClientRect(); return [r.width, r.height];")
        return round(gx * r[0] / GUEST_W - r[0] / 2), round(gy * r[1] / GUEST_H - r[1] / 2)

    def perform(self, actions):
        self.rq("POST", f"/session/{self.sid}/actions", {"actions": actions})
        self.rq("DELETE", f"/session/{self.sid}/actions")

    def move(self, gx, gy, duration=0):
        x, y = self._offset(gx, gy)
        self.perform([{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"},
                       "actions": [{"type": "pointerMove", "duration": duration, "origin": self.canvas(), "x": x, "y": y}]}])

    def wander(self, points, each_ms=120):
        """A path through many points, so a relative mouse has many chances to drift."""
        el = self.canvas()
        acts = []
        for gx, gy in points:
            x, y = self._offset(gx, gy)
            acts.append({"type": "pointerMove", "duration": each_ms, "origin": el, "x": x, "y": y})
        self.perform([{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": acts}])

    def click(self, gx, gy, times=1):
        x, y = self._offset(gx, gy)
        acts = [{"type": "pointerMove", "duration": 0, "origin": self.canvas(), "x": x, "y": y}, {"type": "pause", "duration": 150}]
        for _ in range(times):
            acts += [{"type": "pointerDown", "button": 0}, {"type": "pause", "duration": 60},
                     {"type": "pointerUp", "button": 0}, {"type": "pause", "duration": 90}]
        self.perform([{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": acts}])

    def type(self, text):
        acts = []
        for ch in text:
            v = "\ue007" if ch == "\n" else ch
            acts += [{"type": "keyDown", "value": v}, {"type": "keyUp", "value": v}, {"type": "pause", "duration": KEY_MS}]
        self.perform([{"type": "key", "id": "keys", "actions": acts}])

    # ── where is the GUEST's pointer? ────────────────────────────────────────
    # Measured on the guest's own screen: park the pointer at A, keep that frame, move
    # it to B, and diff. KolibriOS draws its cursor with its hotspot at the arrow's
    # tip (top-left), so the changed pixels around B start at the guest's position.
    def grab(self):
        self.js("window.__grab = document.querySelector('#screen_container canvas').getContext('2d').getImageData(0, 0, 1024, 768).data.slice(); return 1;")

    def cursor_near(self, gx, gy, box=48):
        """Top-left of the pixels that changed since grab(), inside a box around (gx, gy)."""
        return self.js(f"""
          const a = window.__grab, b = document.querySelector('#screen_container canvas').getContext('2d').getImageData(0, 0, 1024, 768).data;
          let minx = 1e9, miny = 1e9, n = 0;
          for (let y = Math.max(0, {gy} - {box}); y < Math.min(768, {gy} + {box}); y++)
            for (let x = Math.max(0, {gx} - {box}); x < Math.min(1024, {gx} + {box}); x++) {{
              const i = (y * 1024 + x) * 4;
              if (a[i] !== b[i] || a[i+1] !== b[i+1] || a[i+2] !== b[i+2]) {{ n++; if (x < minx) minx = x; if (y < miny) miny = y; }}
            }}
          return n ? {{x: minx, y: miny, changed: n}} : null;""")

    def pointer_error(self, start, target, path=()):
        """Move to start, grab, travel `path` then target; distance of the guest cursor from target."""
        self.move(*start)
        time.sleep(0.5)
        self.grab()
        if path:
            self.wander(list(path) + [target])
        else:
            self.move(*target)
        time.sleep(0.6)
        c = self.cursor_near(*target, box=120)
        if not c:
            return None, None
        return max(abs(c["x"] - target[0]), abs(c["y"] - target[1])), c

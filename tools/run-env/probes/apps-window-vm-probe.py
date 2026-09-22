#!/usr/bin/env python3
"""THE APPS WINDOW GATE -- the VM as a real entity-app, its files served by the host.

    GRID=http://127.0.0.1:4471 python3 apps-window-vm-probe.py [spa-url]

Serves nothing itself. Point it at an SPA tree published with the packaged app:

    tools/run-env/package-app.sh
    make site OUT=dist-vm DEPLOY_CONFIG=1 APPS_DIST=tools/run-env/app-dist   (onto a copy of dist/)
    make serve PORT=8211 DIST=dist-vm

What it walks, in the browser-rust UI a person uses: open the Apps window, click
the Alpine card, and wait for the guest's prompt INSIDE the sandboxed player.

WHAT MAKES IT THIS GATE AND NOT THE RIG'S: the page is an opaque-origin srcdoc
frame whose relative URLs resolve against the SPA, where no guest file exists.
So a prompt is only reachable if EVERY boot file came through `x-asset-get` --
and the probe asserts that directly (`assets.mode == host`, served > 0, refused
== 0) rather than inferring it from the boot succeeding, because a page that
fell back to URL mode and found the files some other way would boot too.

Anti-vacuity: the guest must ANSWER a command typed after the prompt, so a page
that paints a fake prompt from cached serial output cannot pass.

THE WORKSPACE HALF (VISITS >= 2). Visit 1 proves the file is ABSENT, writes a
random marker into /root with mode 755, types `save` and `send`, and requires
each script to print the HOST's answer. Visit 2 reloads the same profile and
requires the marker and the mode to be back -- a value the image cannot hold,
because it did not exist before this run. It also checks the control line: the
agent said hello, a window resize reached `stty size`, and no `stty` command
was ever typed into the shell.

THE HOME DIRECTORY, AND POWER (VISITS >= 2). Visit 1 has the host hand the page
two files of one name (and one non-ASCII name): they land in ~, the second
beside the first rather than over it. It then writes a file and does NOT type
`save` -- visit 2 is a reload that kills the frame without warning, so that
file coming back proves the save after each command is what kept it. Visit 2
checks the resumed shell holds visit 1's history and a fresh $RANDOM. After the
last visit (POWER=1, the default): `reboot` typed in the guest restarts it with
a file written just before still there; the menu's "Restart and show the boot"
boots the kernel; "Turn off" stops it; "Start" resumes it.

Exit 0 only if every assertion holds.
"""
import json
import os
import secrets
import sys
import time
import urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8211/?log=trace"
BUDGET_S = float(os.environ.get("BUDGET_S", "240"))

APPS = r"""
  const layer = document.getElementById('dom-layer');
  const root = layer && (layer.shadowRoot || layer);
  let sec = null;
  if (root) for (const s of root.querySelectorAll('section.window')) {
    const h3 = s.querySelector('header h3');
    if (h3 && h3.textContent.trim() === 'Apps') { sec = s; break; }
  }
"""


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


def js(sid, script):
    return rq("POST", f"/session/{sid}/execute/sync", {"script": script, "args": []})["value"]


def poll(sid, script, ok, budget, every=0.5):
    deadline = time.time() + budget
    last = None
    while time.time() < deadline:
        last = js(sid, script)
        if ok(last):
            return last
        time.sleep(every)
    return last


MARK = "MARK-" + secrets.token_hex(6)
STRIP = "replace(/\\x1b\\[[0-9;?]*[a-zA-Z]/g, '')"


def guest(sid, cmd, pattern, budget=60):
    """Type a command; return the first match of `pattern` in its OUTPUT.

    The shell echoes the command line, so neither the done-marker nor the
    pattern may be matched in the echo: the marker is computed by the shell
    (`$((1+1))` prints as 2, echoes as itself), and the pattern is searched only
    after the echo ends.
    """
    import re
    tag = "T" + secrets.token_hex(4)
    js(sid, f"window.__probeFrom = (window.__m1.serial || '').length; "
            f"window.emulator.serial_send_bytes(0, new TextEncoder().encode({json.dumps(cmd + '; echo ' + tag + '-$((1+1))DONE')} + '\\n')); return 1;")
    out = poll(sid, f"const s = (window.__m1.serial || '').slice(window.__probeFrom).{STRIP};"
                    f"return s.includes({json.dumps(tag + '-2DONE')}) ? s : null;",
               lambda v: bool(v), budget) or ""
    echo_end = out.find(tag + "-$((1+1))DONE")
    body = out[echo_end:] if echo_end >= 0 else out
    # `__m1.serial` is a BYTE string (one char per serial byte); read it as the
    # UTF-8 the guest wrote, or every non-ASCII assertion compares mojibake.
    try:
        body = body.encode("latin-1").decode("utf-8", "replace")
    except UnicodeEncodeError:
        pass
    m = re.search(pattern, body)
    return (m.group(0) if m else None), body


checks = []
RANDS = []
BASH_RANDS = []
QUICK = "QUICK-" + secrets.token_hex(5)
def check(name, cond, detail=""):
    checks.append((name, bool(cond)))
    print(f"  {'PASS' if cond else 'FAIL'}  {name:<48} {detail}")


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox",
                                         "moz:firefoxOptions": {"args": ["-headless"]}}}}
print(f"\nAPPS WINDOW GATE -- {URL}  (grid {GRID})\n")
sid = rq("POST", "/session", caps)["value"]["sessionId"]
t0 = time.time()
def visit(n):
        rq("POST", f"/session/{sid}/url", {"url": URL})
        # Phase 2 adopts the deployment's origins, which is where the published
        # catalog comes from. An Apps window opened before it shows only the local
        # catalog -- a race, not the subject -- so wait for the boot to hand over.
        LOG = "return (window.__entity_browser_log || []).map(e => (e.args || []).join(' '));"
        phase2 = poll(sid, LOG, lambda v: any("boot surface down" in l for l in (v or [])), 60)
        check("the boot handed over (phase 2)", any("boot surface down" in l for l in (phase2 or [])))
        opened = poll(sid, r"""
          const layer = document.getElementById('dom-layer');
          const root = layer && (layer.shadowRoot || layer);
          if (!root) return 'no-dom-layer';
          for (const b of root.querySelectorAll('button.spawn-btn'))
            if (b.textContent.trim() === '+ Apps') { b.click(); return 'clicked'; }
          return 'no-btn';""", lambda v: v == "clicked", 60)
        check("opened the Apps window", opened == "clicked", str(opened))

        launched = poll(sid, APPS + r"""
          if (!sec) return 'no-window';
          // A returning profile restores the window with the app already open.
          if (sec.querySelector('iframe[sandbox]')) return 'clicked';
          const b = Array.from(sec.querySelectorAll('button'))
            .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('Alpine Linux'));
          if (!b) return 'no-card:' + sec.textContent.slice(0, 160);
          b.click(); return 'clicked';""", lambda v: v == "clicked", 90)
        check("launched the Alpine card", launched == "clicked", str(launched)[:120])
        t_launch = time.time()

        frame = poll(sid, APPS + r"""
          return sec && sec.querySelector('iframe[sandbox]');""", lambda v: bool(v), 60)
        check("the player mounted a sandboxed frame", bool(frame))
        sandbox = js(sid, APPS + "const f = sec && sec.querySelector('iframe[sandbox]'); return f && f.getAttribute('sandbox');")
        check("the frame is the opaque third-party tier", sandbox and "allow-same-origin" not in sandbox, str(sandbox))

        rq("POST", f"/session/{sid}/frame", {"id": frame})
        inside = poll(sid, "return {href: location.href, title: document.title, m1: !!window.__m1, "
                           "v86: typeof V86, status: (document.getElementById('status')||{}).textContent || null};",
                      lambda v: bool(v) and v.get("m1"), 30)
        print(f"  inside the frame: {json.dumps(inside)}")
        state = poll(sid, "return window.__m1 ? {prompt: window.__m1.promptMs, error: window.__m1.error, "
                          "assets: window.__m1.assets, retain: window.__m1.retain && "
                          "{installed: window.__m1.retain.installed, at: window.__m1.retain.installed_at_ms, "
                          "misses: window.__m1.retain.misses, hits: window.__m1.retain.hits}} : null;",
                     lambda v: bool(v) and (v.get("prompt") or v.get("error")), BUDGET_S, every=1.0)
        t_prompt = time.time()
        a = (state or {}).get("assets") or {}
        check("the page booted in HOST mode", a.get("mode") == "host", json.dumps(a))
        check("the guest reached a prompt", bool(state and state.get("prompt")),
              f"{(state or {}).get('prompt')} ms in-frame · error={(state or {}).get('error')}")
        check("no asset request was refused", a.get("refused") == 0, f"last refusal: {a.get('lastRefusal')}")
        check("the retaining storage seam was installed", bool((state or {}).get("retain", {}).get("installed")),
              json.dumps((state or {}).get("retain")))

        js(sid, "window.emulator.serial_send_bytes(0, new TextEncoder().encode('echo PROBE-$(uname -sm)-$(cat /etc/alpine-release)-OK\\n')); return 1;")
        answered = poll(sid, "const s = (window.__m1.serial || '').replace(/\\x1b\\[[0-9;?]*[a-zA-Z]/g, '');"
                             "const m = s.match(/PROBE-Linux i\\d86-[0-9.]+-OK/); return m ? m[0] : null;",
                        lambda v: bool(v), 30)
        check("the guest ANSWERED a command", bool(answered), str(answered))

        # ── the control line ─────────────────────────────────────────────────
        agent = poll(sid, "return window.__m1.agent || null;", lambda v: bool(v) and (v.get("hello") or v.get("resumed")), 30)
        check("the guest agent is up on ttyS1 (hello, or answered resume)", bool(agent and (agent.get("hello") or agent.get("resumed"))), json.dumps(agent))

        # ── S1: resumed from the snapshot, and what a resume must put right ──
        snap = js(sid, "return window.__m1.snapshot;") or {}
        print(f"  snapshot: used={snap.get('used')} why={snap.get('why')} files={snap.get('files')} rng={snap.get('rng')}")
        if os.environ.get("EXPECT_SNAPSHOT", "1") == "1":
            check("launched by resuming the snapshot", snap.get("used") is True and not snap.get("error"), json.dumps(snap)[:200])
            check("the kernel RNG was reseeded from the page", snap.get("rng") == "reseeded", str(snap.get("rng")))
            check("the console shell was brought up to date in place", snap.get("shell") == "refreshed", str(snap.get("shell")))
            clock, _ = guest(sid, "echo CLK-$(date -u +%s)-", r"CLK-\d+-")
            skew = abs(int(clock[4:-1]) - int(time.time())) if clock else None
            check("the guest clock is today's, not the snapshot's", skew is not None and skew < 120, f"skew {skew} s")
            screen = js(sid, f"return (window.__m1.serial || '').{STRIP}.slice(-400);") or ""
            check("a prompt is on screen after the resume", "$ " in screen[-200:], repr(screen[-80:]))
        else:
            check("booted cold (no snapshot that fits)", snap.get("used") is False, json.dumps(snap)[:200])
        rnd, _ = guest(sid, "echo RND-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \\n')-", r"RND-[0-9a-f]{24}-")
        RANDS.append(rnd)
        if n == 2:
            check("the two visits read different random bytes", len(RANDS) == 2 and RANDS[0] and RANDS[0] != RANDS[1], str(RANDS))
        # The console shell itself: one started at snapshot BUILD time hands every
        # visitor the same $RANDOM sequence and none of their history.
        br, _ = guest(sid, "echo BR-$RANDOM-$RANDOM-", r"BR-\d+-\d+-")
        BASH_RANDS.append(br)
        if n == 2:
            check("the two visits' shells give different $RANDOM", len(BASH_RANDS) == 2 and BASH_RANDS[0] and BASH_RANDS[0] != BASH_RANDS[1], str(BASH_RANDS))
            # [x] so the check command's own history line does not match itself.
            pat = MARK[:5] + "[" + MARK[5] + "]" + MARK[6:]
            hist, body = guest(sid, f"echo HIST-$(history | grep -c '{pat}')-", r"HIST-\d+-")
            check("the resumed shell holds the last visit's history", bool(hist) and hist != "HIST-0-", f"{hist} ({pat})")
        before = js(sid, "const f = window.__m1.fit; return f[f.length-1];")
        rq("POST", f"/session/{sid}/window/rect", {"width": 900 + 40 * n, "height": 700})
        time.sleep(1.5)
        after = js(sid, "const f = window.__m1.fit; return f[f.length-1];")
        size, _ = guest(sid, "echo SZ-$(stty size)-", r"SZ-\d+ \d+-")
        want = f"SZ-{after['rows']} {after['cols']}-"
        check("a resize reached the guest's console size", size == want, f"guest {size} page {want} (was {before['rows']}x{before['cols']})")
        typed = js(sid, f"return /stty rows/.test((window.__m1.serial || '').{STRIP});")
        check("no stty command was typed into the shell", typed is False, f"typed={typed}")

        # ── the workspace ────────────────────────────────────────────────────
        if n == 1:
            present, _ = guest(sid, "test -e /root/proj/note.txt && echo WS-PRESENT || echo WS-ABSENT", r"WS-(PRESENT|ABSENT)")
            check("a fresh profile has no saved file (anti-vacuity)", present == "WS-ABSENT", str(present))
            wrote, _ = guest(sid, f"mkdir -p /root/proj && printf {MARK} > /root/proj/note.txt && chmod 755 /root/proj/note.txt && echo WS-WROTE", r"WS-WROTE")
            check("wrote a marker into /root", wrote == "WS-WROTE", MARK)
            said, out = guest(sid, "save", r"save: [^\r\n]*", 120)
            check("`save` printed the host's answer", bool(said) and "saved" in said and "kept in this browser" in said, str(said))
            sent, out = guest(sid, "send /root/proj/note.txt", r"send: [^\r\n]*", 90)
            check("`send` printed the host's answer", bool(sent) and "handed to the page" not in sent and "note.txt" in sent, str(sent))
            # A non-ASCII name crosses the OSC, the host and the control line intact,
            # and the host's bidi isolates do not reach the terminal.
            guest(sid, "printf x > /root/proj/café.txt", r"")
            sent2, _ = guest(sid, "send /root/proj/café.txt", r"send: [^\r\n]*", 90)
            check("a non-ASCII file name survives the round trip", bool(sent2) and "café.txt" in sent2
                  and not any(c in sent2 for c in "\u2066\u2067\u2068\u2069\ufffd"), repr(sent2))
            # M2: the player bar offers "Save to this device" for the file just sent,
            # and it hands the browser that file's bytes. Captured at the object URL
            # and the anchor, not performed -- a download on a grid is a dialog.
            rq("POST", f"/session/{sid}/frame/parent", {})
            pressed = poll(sid, APPS + r"""
                const b = sec && sec.querySelector('[data-field="app-file-save"]');
                if (!b || b.hidden || b.getAttribute('data-file-name') !== 'café.txt') return null;
                window.__ftSaved = [];
                const blobs = new Map();
                if (!URL.__ftOrig) URL.__ftOrig = URL.createObjectURL;
                URL.createObjectURL = function (x) { const u = URL.__ftOrig.call(URL, x); blobs.set(u, x); return u; };
                HTMLAnchorElement.prototype.click = function () {
                  const x = blobs.get(this.href);
                  const rec = { download: this.getAttribute('download'), text: null };
                  window.__ftSaved.push(rec);
                  if (x) x.text().then(t => { rec.text = t; });
                };
                b.click();
                return 'clicked';""", lambda v: v == "clicked", 30)
            check("the player offers Save to this device for the sent file", pressed == "clicked", str(pressed))
            saved = poll(sid, "const r = (window.__ftSaved || [])[0]; return r && r.text !== null ? r : null;",
                         lambda v: bool(v), 30)
            check("Save to this device hands the browser the sent file's bytes",
                  bool(saved) and saved.get("download") == "café.txt" and saved.get("text") == "x", str(saved))
            rq("POST", f"/session/{sid}/frame", {"id": frame})
            work = js(sid, "return window.__m1.work;")
            print(f"  workspace (page): {json.dumps(work)[:300]}")

            # ── M4: the package set ──────────────────────────────────────────
            pk = js(sid, "return {pk: window.__m1.packages, b: window.__m1.assets.byBundle.packages || null};")
            print(f"  packages: {json.dumps(pk)}")
            check("the package set was merged into the root", bool(pk and pk.get("pk") and pk["pk"].get("files", 0) > 10
                  and not pk["pk"].get("error")), json.dumps(pk))
            before_n = ((pk or {}).get("b") or {}).get("requests", 0)
            absent, _ = guest(sid, "command -v jq >/dev/null && echo PK-HAVE || echo PK-NONE", r"PK-(HAVE|NONE)")
            check("jq is not in the image (anti-vacuity)", absent == "PK-NONE", str(absent))
            # Signature enforcement: with no trusted keys apk must refuse OUR index.
            # (apk skips an index it cannot verify with a warning, then reports the
            # package as missing -- so require BOTH the UNTRUSTED line and no jq.)
            untrusted, body = guest(sid, "mkdir -p /tmp/nokeys; apk add --no-cache --keys-dir /tmp/nokeys jq >/tmp/apk-nokeys.log 2>&1; "
                                         "echo PK-RC-$?; grep -i -m1 untrusted /tmp/apk-nokeys.log; command -v jq || echo PK-STILL-NONE",
                                    r"(?is)UNTRUSTED.*PK-STILL-NONE", 180)
            check("apk refuses the repository without our key", bool(untrusted), repr(body[-240:]))
            added, body = guest(sid, "apk add --no-cache jq >/tmp/apk.log 2>&1; echo PK-ADD-$?; tail -2 /tmp/apk.log",
                                r"PK-ADD-\d+", 300)
            check("apk add jq succeeds from the package set", added == "PK-ADD-0", repr(body[-300:]))
            ver, _ = guest(sid, "echo '{\"v\":42}' | jq -r .v | sed 's/^/PK-JQ-/'", r"PK-JQ-\d+")
            check("the installed jq runs", ver == "PK-JQ-42", str(ver))
            after = js(sid, "return window.__m1.assets.byBundle.packages || null;") or {}
            fetched = after.get("requests", 0) - before_n
            check("only what the install needs was fetched from the set", 0 < fetched < 12,
                  f"{fetched} package files for jq ({after.get('bytes', 0) / 1048576:.1f} MiB total from the bundle) "
                  f"of {pk['pk'].get('files') if pk and pk.get('pk') else '?'}")

            # ── the save after each command: no `save` typed ─────────────────
            wrote_q, _ = guest(sid, f"printf {QUICK} > ~/quick.txt && echo Q-WROTE", r"Q-WROTE")
            # The save that carried THIS file, not any save: an earlier command's
            # save satisfied "a prompt save happened" and the next visit then
            # found no file (the first run of this check). Before the receive
            # steps, because their saves queue behind each other and the last one
            # picked this file up under reason "received" (measured).
            quick = poll(sid, "const w = window.__m1.work; return w && (w.recent || []).find(r => r.reason === 'prompt' && "
                              "(r.paths || []).includes('quick.txt')) || null;", bool, 20)
            recent = js(sid, "return window.__m1.work.recent;")
            check("a command's change is saved within seconds, unasked", wrote_q and bool(quick),
                  json.dumps(quick) if quick else json.dumps(recent)[:400])
            # The host's tree is write-behind (250 ms debounce on the Direct/IDB
            # arm), and the next visit RELOADS THE WHOLE SPA -- the one close that
            # can outrun that flush (measured: a reload ~1 s after the save lost
            # it once). Closing only the app window leaves the SPA running, so
            # the flush completes; this wait keeps the gate on the save, not on
            # the host's durability window.
            time.sleep(2)

            # ── receive: into the home directory, never over a file ──────────
            # Posted as the host would after its "Send a file" picker (the picker
            # itself is the x-file e2e gate's subject, not this one's).
            def push(name, text):
                rq("POST", f"/session/{sid}/frame/parent", {})
                js(sid, APPS + f"""
                  const f = sec.querySelector('iframe[sandbox]');
                  const data = new TextEncoder().encode({json.dumps(text)}).buffer;
                  f.contentWindow.postMessage({{source: 'entity-host', type: 'x-file', name: {json.dumps(name)},
                    media_type: 'text/plain', data}}, '*', [data]); return 1;""")
                rq("POST", f"/session/{sid}/frame", {"id": frame})
            push("some/dir/notes.txt", "first")
            got1 = poll(sid, "const r = (window.__m1.files || {}).received || []; return r.length >= 1 ? r : null;", bool, 30)
            push("notes.txt", "second")
            push("my café.txt", "accented")
            got = poll(sid, "const r = (window.__m1.files || {}).received || []; return r.length >= 3 ? r : null;", bool, 30) or []
            dests = [g.get("dest") for g in got]
            check("a received file lands in ~ under its base name", len(got) >= 1 and got[0].get("dest") == "notes.txt", json.dumps(got1))
            check("a second file of that name lands beside it, not over it", dests[1:2] == ["notes-1.txt"], json.dumps(got))
            check("a non-ASCII name with a space is received intact", dests[2:3] == ["my café.txt"], json.dumps(got))
            both, body = guest(sid, "echo RX-$(cat ~/notes.txt)-$(cat ~/notes-1.txt)-$(cat ~/'my café.txt')-", r"RX-[^\r\n$]*-")
            check("the guest reads both files and neither was overwritten", both == "RX-first-second-accented-", repr(both))
            gone, _ = guest(sid, "echo MNT-$(ls -A /mnt | wc -l | tr -d ' ')-", r"MNT-\d+-")
            check("nothing landed in /mnt", gone == "MNT-0-", str(gone))

        else:
            work = js(sid, "return window.__m1.work;")
            print(f"  workspace (page): {json.dumps(work)[:300]}")
            # Fetched, not placed: the marker check below is what proves the restore
            # (falsified -- with placeRestore neutered this one stays green).
            check("the page fetched the saved files from the host", (work or {}).get("restored", 0) >= 1, json.dumps(work)[:200])
            back, _ = guest(sid, "echo WS-$(cat /root/proj/note.txt)-$(stat -c %a /root/proj/note.txt)-", r"WS-[^\r\n]*-")
            check("the marker came back, with its mode", back == f"WS-{MARK}-755-", f"got {back} want WS-{MARK}-755-")
            rx, _ = guest(sid, "echo RX-$(cat ~/notes.txt)-$(cat ~/notes-1.txt)-$(cat ~/'my café.txt')-", r"RX-[^\r\n$]*-")
            check("received files came back with the home directory", rx == "RX-first-second-accented-", repr(rx))
            qb, _ = guest(sid, "echo QB-$(cat ~/quick.txt)-", r"QB-[^\r\n$]*-")
            check("the unasked save survived a close with no warning", qb == f"QB-{QUICK}-", f"got {qb} want QB-{QUICK}-")
            # packs (build-guest.sh 3c3): a named tool set, as the line it would install.
            pk, _ = guest(sid, "packs show c", r"apk add [^\r\n]*")
            check("packs names the C tool set", pk == "apk add tcc tcc-libs-static musl-dev make", repr(pk))
            # ...and it WORKS: the set installs offline, and tcc compiles, LINKS and runs a
            # program. Linking is the half that fails on a bare Alpine 3.22 x86 tcc (it looks
            # for crt1.o under /usr/lib/i386-linux-gnu; build-guest.sh 3c4 puts it there).
            cc, body = guest(sid, "packs add c >/tmp/packs.log 2>&1; echo PK-C-$?; "
                                  "printf '#include <stdio.h>\\nint main(void){printf(\"CC-%%d-OK\\\\n\", 6*7);return 0;}\\n' > /tmp/h.c; "
                                  "tcc -o /tmp/h /tmp/h.c && /tmp/h", r"CC-42-OK", 300)
            check("packs add c installs tcc, and tcc compiles, links and runs C", cc == "CC-42-OK", repr(body[-400:]))
            # ⓘ: the component table, then the package rows read from what shipped
            # (about.json, fetched through the host the first time the panel opens).
            js(sid, "document.getElementById('info').click(); return 1;")
            about = poll(sid, """const p = document.getElementById('aboutpanel');
              const rows = p.querySelectorAll('tbody[data-filterable] tr').length;
              return p.hidden ? null : {rows, text: p.textContent.slice(0, 4000)};""",
              lambda v: bool(v) and v["rows"] > 0, 30) or {}
            check("ⓘ lists the kernel's licence and the shipped packages", "GPL-2.0-only" in (about.get("text") or "") and about.get("rows", 0) >= 297,
                  f"rows={about.get('rows')}")
            js(sid, "document.body.click(); return 1;")

        final_assets = js(sid, "return window.__m1.assets;")
        rq("POST", f"/session/{sid}/frame/parent", {})
        all_lines = js(sid, LOG) or []
        ready_lines = [l for l in all_lines if "app assets: bundle ready" in l]
        for line in [l for l in all_lines if "app workspace:" in l][-4:]:
            print("  log: " + " ".join(p.strip() for p in line.split("\n"))[:160])
        for line in ready_lines:
            print("  log: " + " ".join(p.strip() for p in line.split("\n") if "=" in p)[:160])
        # One settle per bundle per launch. Two means the player was mounted twice --
        # the app restarted under itself (the watch-on-assets defect).
        import re as _re
        settled = [m.group(1) for l in ready_lines for m in [_re.search(r"bundle = (\w+)", l)] if m]
        declared = set((final_assets.get("byBundle") or {}).keys())
        check("each bundle settled exactly once (no restart)",
              len(settled) == len(set(settled)) and set(settled) >= declared and len(settled) >= 2,
              f"{len(ready_lines)} 'bundle ready' lines: {sorted(settled)}; the page asked {sorted(declared)}")
        stamps = js(sid, APPS + r"""
          const f = sec && sec.querySelector('iframe[sandbox]');
          return f && { saves: f.getAttribute('data-app-work-saves'),
                        served: f.getAttribute('data-app-assets-served'),
                        refused: f.getAttribute('data-app-assets-refused'),
                        last: f.getAttribute('data-app-assets-last-refusal') };""")
        check("the host stamps agree with the page", stamps and str(stamps.get("served")) == str(final_assets.get("requests"))
              and not stamps.get("refused"), f"host={stamps} page requests={final_assets.get('requests')}")

        net = js(sid, r"""
          const rs = performance.getEntriesByType('resource');
          const pick = f => { const m = rs.filter(f); return { n: m.length,
            bytes: m.reduce((a, r) => a + (r.transferSize || 0), 0) }; };
          return { pointers: pick(r => r.name.endsWith('.bin')),
                   content: pick(r => r.name.includes('/content/')),
                   guest_urls: pick(r => /vmlinuz|initramfs|fs\.json|\/blobs\//.test(r.name)),
                   total: pick(() => true) };""")
        print(f"\n  network (top page): {json.dumps(net)}")
        print(f"  assets through the host: {final_assets.get('requests')} requests, "
              f"{final_assets.get('bytes', 0) / 1048576:.1f} MiB, fixed files in {final_assets.get('fixed_ms')} ms")
        print(f"  [visit {n}] launch -> prompt: {t_prompt - t_launch:.1f} s   (session total {time.time() - t0:.1f} s)")
        check("no guest file was fetched by URL", net["guest_urls"]["n"] == 0, json.dumps(net["guest_urls"]))


def power_tests():
    """reboot in the guest, then the menu: restart showing the boot, off, start.

    Runs inside the player frame the last visit left us in. A restart RELOADS the
    frame's document, so every read is a fresh document's `window.__m1`, and a
    read that lands mid-reload is retried rather than failed.
    """
    print("\n-- power --")
    APPS_FRAME = APPS + "return sec && sec.querySelector('iframe[sandbox]');"
    def into_frame():
        rq("POST", f"/session/{sid}/frame/parent", {})
        f = js(sid, APPS_FRAME)
        rq("POST", f"/session/{sid}/frame", {"id": f})
    def safe(script):
        try:
            return js(sid, script)
        except Exception:
            try:
                into_frame()
            except Exception:
                pass
            return None
    def wait_started(by, boot, budget):
        deadline = time.time() + budget
        v = None
        while time.time() < deadline:
            v = safe("const m = window.__m1; return m && m.power && m.power.started.by === " + json.dumps(by) +
                     " && m.power.started.boot === " + json.dumps(boot) + " && m.promptMs ? "
                     "{started: m.power.started, snap: m.snapshot, prompt: m.promptMs, agent: m.agent, "
                     "serial: (m.serial || '').slice(0, 4000)} : null;")
            if v:
                return v
            time.sleep(1)
        return v
    def click_power(item):
        return js(sid, "document.getElementById('power').click();"
                       f"const b = document.querySelector('#powermenu [data-power={json.dumps(item)}]');"
                       "if (!b || b.hidden || document.getElementById('powermenu').hidden) return 'no-item';"
                       "b.click(); return 'clicked';")

    # 1. `reboot` typed in the guest, with a file written on the same line: only
    #    the power action's own save can have kept it.
    RB = "RB-" + secrets.token_hex(5)
    js(sid, "window.emulator.serial_send_bytes(0, new TextEncoder().encode(" +
            json.dumps(f"printf {RB} > ~/before-reboot.txt && reboot\n") + ")); return 1;")
    time.sleep(2)
    r1 = wait_started("command", "resume", 120)
    check("`reboot` in the guest restarts the machine (resumed)", bool(r1) and r1["snap"].get("used") is True,
          json.dumps((r1 or {}).get("started")))
    got, _ = guest(sid, "echo RBK-$(cat ~/before-reboot.txt)-", r"RBK-[^\r\n$]*-") if r1 else (None, "")
    check("a file written just before `reboot` survived it", got == f"RBK-{RB}-", f"got {got}")
    note = safe("return window.__m1.power.started.note;")
    check("the restarted terminal says the home directory was saved", note == "restarted -- your home directory was saved", str(note))

    # 2. The menu: restart and show the boot.
    c = click_power("cold")
    check("the power menu offers Restart and show the boot", c == "clicked", str(c))
    time.sleep(2)
    r2 = wait_started("menu", "cold", BUDGET_S)
    check("Restart and show the boot boots the kernel (no snapshot)", bool(r2) and r2["snap"].get("used") is False
          and "Linux version" in r2.get("serial", ""), json.dumps({k: (r2 or {}).get(k) for k in ("started", "snap", "prompt")})[:300])
    back, _ = guest(sid, "echo WS2-$(cat ~/before-reboot.txt)-", r"WS2-[^\r\n$]*-") if r2 else (None, "")
    check("the home directory is there after a boot from scratch", back == f"WS2-{RB}-", f"got {back}")

    # 3. Turn off, then Start.
    c = click_power("off")
    check("the power menu offers Turn off", c == "clicked", str(c))
    off = poll(sid, "const m = window.__m1; const a = m.power.actions; const l = a[a.length - 1];"
                    "return l && l.what === 'off' && l.done ? {emu: !!window.emulator, cls: document.getElementById('power').className,"
                    "start: !document.querySelector('#powermenu [data-power=start]').hidden,"
                    "restart: !document.querySelector('#powermenu [data-power=restart]').hidden,"
                    "screen: document.querySelector('.xterm-rows').innerText.slice(-300)} : null;", bool, 60)
    check("Turn off stops the machine and says so", bool(off) and off["emu"] is False and "The machine is off" in off["screen"]
          and off["start"] and not off["restart"], json.dumps(off)[:300])
    c = click_power("start")
    check("the power menu offers Start once off", c == "clicked", str(c))
    time.sleep(2)
    r3 = wait_started("menu", "resume", 120)
    check("Start resumes the machine", bool(r3) and r3["snap"].get("used") is True, json.dumps((r3 or {}).get("started")))
    ans, _ = guest(sid, "echo ON-$(cat ~/before-reboot.txt)-", r"ON-[^\r\n$]*-") if r3 else (None, "")
    check("the machine answers after Start, home directory intact", ans == f"ON-{RB}-", f"got {ans}")


try:
    for n in range(1, int(os.environ.get("VISITS", "1")) + 1):
        print(f"\n-- visit {n} --")
        visit(n)
    if int(os.environ.get("VISITS", "1")) >= 2 and os.environ.get("POWER", "1") == "1":
        rq("POST", f"/session/{sid}/frame", {"id": js(sid, APPS + "return sec && sec.querySelector('iframe[sandbox]');")})
        power_tests()
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

failed = [n for n, ok in checks if not ok]
print("\n*** THE VM RUNS AS AN APP, ITS FILES SERVED BY THE HOST ***" if not failed
      else f"\n*** {len(failed)} FAILED: {', '.join(failed)} ***")
sys.exit(1 if failed else 0)

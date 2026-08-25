#!/usr/bin/env python3
"""MEET THEN CHAT — the whole product claim in one run: **name -> id -> connection -> message.**

`spike_chat_over_webrtc.py` proves the *mechanism*: two browsers chat over a real
RTCDataChannel. It proves it by handing each browser the other's 44-character peer
id and the signaling node as URL params, with `?webrtc_enable=1`. Every one of
those is something a user does not have and cannot type.

This spike removes all three. Nothing is passed in the URL. Instead:

  1. Both browsers boot **bare** — no `?webrtc_node`, no `?webrtc_enable`. At this
     point neither has a signaling node and neither installs a §6.5 establisher.
     That is the correct cold state, and it is asserted, not assumed.
  2. Each adds the node to its **durable connector registry** through the Shell
     (`connector add` / `connector use`) — the surface a user actually has.
  3. Both **reload**. Provisioning now resolves from the selected connector, and
     the establisher installs off the user's own choice. This is the step that a
     build-time enable knob made impossible, and the log line asserting it
     (`provisioning from the selected connector`) is the regression gate for it.
     The reload is load-bearing, not incidental: `InitParams.webrtc` is Init-only
     upstream, so a selection made mid-session cannot reach a running establisher.
  4. Both `meet tag <label>` — the same name, typed on both sides. Each comes away
     with a peer id it was never told.
  5. Chat binds **the discovered id**, and a message crosses both ways.

The assertion that makes step 4 mean something: the id A learned must equal B's
actual peer id, and vice versa. A meet that reported *something* while the ids
came from anywhere else would otherwise pass.

Preconditions are `rung1_repro.sh`'s (rtc-a :4446, rtc-b :4447 on the shared
bridge, dist on host :8092, node on host :4071). arg1 is the node peer id.

MODE (env, default `direct`) is the same axis as the chat spike: `direct` is the
shipped Direct/IDB arm, `worker` the opt-in Worker/OPFS arm.

## Every UI action is confirmed, never assumed — and that is the whole design

An earlier version submitted each Shell command **once** and read the result after
a fixed sleep. It failed at step 4 on nearly every run, looking exactly like an app
freeze: the Shell took a line and printed nothing. It was not an app freeze, and
the app was never at fault.

**The cause, measured attempt by attempt:**

    attempt 1: windows=0  input_found=False  type='no-field:shell-input'  landed=False
    attempt 2: windows=1  input_found=True   type='typed'                 landed=True

Clicking "+ Shell" only *queues* an action; the window is created on the frame
after. A harness that types in the same instant finds **no input element at all**,
so nothing is submitted — and then reports the command as ignored. Controls:
with a 2s pause after the spawn, the first attempt lands; on an already-open
Shell, the first attempt lands. The first real submit has never failed.

(An intermediate version of this note claimed "the first submit into a freshly
spawned Shell does not land." That was wrong — no submit occurred — and the
distinction matters: one is an app defect, the other is this file racing a frame.
`type_until` now counts only attempts that actually typed, so the printed number
cannot imply the app swallowed anything.)

So the fix is structural, not a patch on a symptom: **every interaction retries
until the app visibly responds**, and every confirmation comes from the app's own
output rather than from the fact that a keystroke was dispatched.

  - `run_until` / `type_until` — re-submit until a stated condition holds.
  - `until_listed` — confirm a `connector` write via a SEPARATE `connector ls`,
    because the Shell echoes the line it was given and asserting on the echo
    passes whether or not the verb ran.
  - the Chat bind is confirmed by the start-by-id input **disappearing** (it
    renders only while unbound), not by a messages element merely existing.
  - `send_and_wait` sends once and gives delivery a real window, rather than
    retyping a message every second into a live conversation.

Ruled out earlier, with a measurement each, so nobody re-walks them: duplicate
windows (counted), a frame panic (no `panicked at`/`FRAME PANIC`), an
unhandled-rejection reload loop (a `window` marker survives), a per-frame write
storm (0.0/s), a dead frame loop (spawns still land, and the log flatlines
*before* the reload too, where the Shell works — a flat log is just idle), the
action being dropped app-side (`app.rs` warns on that now; it never fired), the
typing method, the log level, browser concurrency, and time since boot.

A fixed sleep followed by an assert encodes a guess about how long an async
re-render takes — the shape AGENTS names as the systemic source of load-dependent
flake, and the reason the suite prefers `poll_json` over `sleep(fixed)`. This file
is that lesson applied to a WebDriver harness.
"""
import json, os, re, sys, time, urllib.request

MODE = os.environ.get("MODE", "direct").strip().lower()
if MODE not in ("direct", "worker"):
    print(f"!! MODE must be 'direct' or 'worker', got {MODE!r}"); sys.exit(2)

# The name both sides meet at. A `tag` is the public mode — exactly right here,
# since the point is that two strangers who agree on a word find each other.
TAG = os.environ.get("MEET_TAG", "rung1-chess").strip()

# Negative control. Set by the split-network rig (`TOPOLOGY=split`), where the
# two browsers sit on isolated podman networks with no route between them and
# reach the signaling node only through the host. Inverts the MEDIA half of the
# gate: rendezvous must still work, delivery must not. See the block at the end
# of `main` for exactly what it asserts and why passing means a real limit.
EXPECT_NO_MEDIA = os.environ.get("EXPECT_NO_MEDIA", "") == "1"

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

BOOT = ("const l=document.getElementById('dom-layer');const r=l&&(l.shadowRoot||l);"
        "return r&&r.querySelector('button.spawn-btn')?'booted':'no';")

def spawn_script(label):
    return ("const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;"
            "for(const b of r.querySelectorAll('button.spawn-btn')){"
            f"if(b.textContent.trim()==='+ {label}'){{b.click();return 'clicked';}}}}"
            "return 'no-btn';")

def _windows(title):
    """Collect every open window titled `title` into `out`, in DOM order."""
    return ("const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;"
            "const out=[];for(const sec of r.querySelectorAll('section.window')){"
            "const h=sec.querySelector('header h3');"
            f"if(h&&h.textContent.trim()==='{title}')out.push(sec);}}")

def field_text(title, field):
    """textContent of `[data-field]` in the LAST window titled `title`."""
    return _windows(title) + ("if(!out.length)return '(no-window)';"
                              f"const e=out[out.length-1].querySelector('[data-field=\"{field}\"]');"
                              "return e?e.textContent:'(no-field)';")

SHELL_TEXT = field_text("Shell", "shell-scrollback")
CHAT_TEXT = field_text("Chat", "chat-messages")
REACH_TEXT = field_text("Chat", "chat-reachability")
SHELL_COUNT = _windows("Shell") + "return out.length;"

def ex(base, sid, script, args=None):
    return rq(base, "POST", f"/session/{sid}/execute/sync",
              {"script": script, "args": args or []})["value"]

# ---------------------------------------------------------------------------
# Typing — real WebDriver keys, and always confirmed
# ---------------------------------------------------------------------------
W3C_ELEM = "element-6066-11e4-a52e-4f735466cecf"
ENTER = "\ue007"

def find_field(base, sid, title, field):
    script = _windows(title) + ("if(!out.length)return null;"
                                f"return out[out.length-1].querySelector('[data-field=\"{field}\"]');")
    el = ex(base, sid, script)
    return el.get(W3C_ELEM) if isinstance(el, dict) and W3C_ELEM in el else None

def type_once(base, sid, title, field, text, submit=True):
    """One attempt. A stale element mid-rebuild is an expected outcome, not an error."""
    eid = find_field(base, sid, title, field)
    if not eid:
        return f"no-field:{field}"
    try:
        rq(base, "POST", f"/session/{sid}/element/{eid}/click", {})
        rq(base, "POST", f"/session/{sid}/element/{eid}/clear", {})
        rq(base, "POST", f"/session/{sid}/element/{eid}/value",
           {"text": text + (ENTER if submit else "")})
        return "typed"
    except Exception as e:
        return f"stale:{type(e).__name__}"

def type_until(base, sid, title, field, text, done, tries=20, gap=1.0, label=""):
    """Type `text` into the field until `done()` holds.

    Returns `(ok, submits)` where `submits` counts attempts that actually typed
    something — an attempt that found no field typed nothing and must not be
    reported as a submit, or the count reads as "the app ignored a command" when
    the truth is "there was nothing to type into yet." Getting that distinction
    wrong is what produced a false finding once already; see the module doc.

    `done` is checked before the first attempt, so an already-satisfied
    condition costs nothing.
    """
    submits = 0
    for _ in range(tries):
        if done():
            return True, submits
        if type_once(base, sid, title, field, text, submit=True) == "typed":
            submits += 1
        time.sleep(gap)
    return done(), submits

def run_until(base, sid, cmd, done, tries=20, gap=1.0):
    """A Shell command, re-submitted until the app visibly responds."""
    return type_until(base, sid, "Shell", "shell-input", cmd, done, tries, gap)

def shell_says(base, sid, needle):
    return lambda: needle in (ex(base, sid, SHELL_TEXT) or "")

def shell_grew(base, sid):
    """Truthy once the scrollback holds anything beyond the empty placeholder."""
    start = ex(base, sid, SHELL_TEXT) or ""
    def done():
        now = ex(base, sid, SHELL_TEXT) or ""
        return now != start and "(scrollback cleared)" not in now
    return done

# ---------------------------------------------------------------------------
# Session plumbing
# ---------------------------------------------------------------------------
def goto(base, sid):
    """Load the app with NOTHING provisioned in the URL — the shipped path."""
    worker = "?worker=1&" if MODE == "worker" else "?"
    rq(base, "POST", f"/session/{sid}/url", {"url": f"{APP}/{worker}log=debug"})

def new_session(base):
    sid = rq(base, "POST", "/session", CAPS)["value"]["sessionId"]
    rq(base, "POST", f"/session/{sid}/timeouts", {"script": 30000})
    goto(base, sid)
    return sid

def wait_boot(base, sid, label):
    for i in range(60):
        time.sleep(1)
        if ex(base, sid, BOOT) == "booted":
            print(f"  {label} booted ({i+1}s)"); return True
    print(f"  {label} FAILED to boot"); return False

def log_lines(base, sid):
    return ex(base, sid, "return (window.__entity_browser_log||[]).map(e=>JSON.stringify(e))") or []

def log_has(base, sid, needle):
    n = needle.lower()
    return any(n in l.lower() for l in log_lines(base, sid))

def open_shell(base, sid, label):
    """Ensure a usable Shell exists and has actually printed its banner.

    Waiting for the window before typing is not belt-and-braces: a spawn click
    only *queues* an action, and the window is created on the frame after. Typing
    in the same instant finds no input element at all — which is a harness race,
    not the app dropping anything, and it must not be dressed up as one.
    """
    if not ex(base, sid, SHELL_COUNT):
        ex(base, sid, spawn_script("Shell"))
        for _ in range(30):
            if ex(base, sid, SHELL_COUNT):
                break
            time.sleep(0.5)
    ok, submits = run_until(base, sid, "info", shell_says(base, sid, "bound peer"))
    print(f"  {label} Shell ready: {ok} (after {submits} submit(s))")
    return ok

BOUND_ROW = re.compile(r"bound peer[^\n]*?([1-9A-HJ-NP-Za-km-z]{40,})")

def bound_peer_id(base, sid, label):
    """The Shell's bound peer, from `info` — the id a counterpart meets."""
    ok, _ = run_until(base, sid, "info", shell_says(base, sid, "bound peer"))
    m = BOUND_ROW.search(ex(base, sid, SHELL_TEXT) or "") if ok else None
    if not m:
        print(f"  {label} could not read its bound peer; scrollback: "
              f"{(ex(base, sid, SHELL_TEXT) or '')[-300:]!r}")
        for line in log_lines(base, sid):
            if any(k in line for k in ("panicked at", "FRAME PANIC", "DROPPED")):
                print(f"  {label} !! {line[:400]}")
        return None
    return m.group(1)

def short_pid(pid):
    """How the app abbreviates a peer-id in scrollback (`views::short_pid`)."""
    return f"{pid[:8]}...{pid[-6:]}" if len(pid) > 16 else pid

def until_listed(base, sid, cmd, marker, tries=10):
    """Run `cmd`, then re-read `connector ls`, until `marker` shows up.

    The confirmation deliberately comes from a SEPARATE `connector ls` rather
    than from the command's own output: the Shell echoes the line it was given,
    so asserting on the echo passes whether or not the verb ran. `clear` first,
    so a marker left by an earlier iteration cannot be mistaken for this one's.
    Writes are dispatched (they land a dispatch later), so this is also what
    waits for the tree rather than guessing a sleep.
    """
    for _ in range(tries):
        type_once(base, sid, "Shell", "shell-input", cmd)
        time.sleep(1.0)
        type_once(base, sid, "Shell", "shell-input", "clear")
        time.sleep(0.6)
        type_once(base, sid, "Shell", "shell-input", "connector ls")
        time.sleep(1.0)
        if marker in (ex(base, sid, SHELL_TEXT) or ""):
            return True
    return False

# Reflectors for the connector row, when the rig is driving a NAT topology.
# Empty (the default) reproduces every gate exactly as before: host candidates
# only, which is what a shared bridge needs and all any gate has ever used.
E2E_ICE = os.environ.get("E2E_ICE", "").strip()

# The AUTOMATIC half (EXTENSION-SIGNALING §4.5.1): the node was started with
# `--reflection-endpoint` and publishes this in `advertise`. The browsers are
# handed NOTHING — they learn it by asking, which is the entire point. Set by
# the rig, never typed into the Shell.
E2E_NODE_REFLECTION = os.environ.get("E2E_NODE_REFLECTION", "").strip()

def expected_ice_urls():
    """The §4.5.1 merge, as an expectation — counted in URLs, not entries.

    A consumer merges rather than replaces, deduplicated by endpoint bytes
    *exactly as published*. So typed-only ⇒ 1, advertised-only ⇒ 1, both with
    the SAME uri ⇒ 1 (the dedup is doing work), both DIFFERENT ⇒ 2 (the union
    is doing work). Encoding the rule rather than a constant is what lets one
    run prove dedup and another prove union.

    **Counted on `ice_urls`, not `ice_servers`.** `parse_ice_urls` packs every
    reflector into ONE `IceServer`, so `ice_servers` is 0-or-1 by construction
    and can only ever say "configured or not" — it cannot see a merge. Asserting
    the merge against it would pass for any non-empty list, which is exactly the
    volume-blind shape §11.5.1 warns about.
    """
    typed = [E2E_ICE] if E2E_ICE else []
    advertised = [a for a in ([E2E_NODE_REFLECTION] if E2E_NODE_REFLECTION else [])
                  if a not in typed]
    return len(typed + advertised)

def provision(base, sid, node_peer, label):
    """Add + select the connector through the Shell — the user's own surface."""
    open_shell(base, sid, label)
    # `ice=` rides the SAME `connector add` a user types. Deliberately not
    # injected by URL: the durable registry row is the shipped path, and it is
    # the one that has to carry reflectors all the way to the ICE agent.
    ice_arg = f" ice={E2E_ICE}" if E2E_ICE else ""
    ok_add = until_listed(base, sid, f"connector add {node_peer} {NODE_WS}{ice_arg} rung1",
                          short_pid(node_peer))
    # `connector ls` marks the selection with ●.
    ok_use = until_listed(base, sid, f"connector use {node_peer}", "●")
    print(f"  {label} connector added: {ok_add}   selected: {ok_use}")
    return ok_add and ok_use

# `met <short>  <full-id>` is what pump_meet pushes per discovered peer.
MET_RE = re.compile(r"met\s+\S+\s+([1-9A-HJ-NP-Za-km-z]{40,})")

def met_ids(base, sid):
    return MET_RE.findall(ex(base, sid, SHELL_TEXT) or "")

def send_and_wait(from_base, from_sid, to_base, to_sid, msg, label,
                  sends=3, wait_each=20):
    """Send `msg` and wait for it to appear on the other side.

    Deliberately NOT `type_until`: retrying the *send* every second would post
    twenty copies of the same line into a conversation while the first is still
    crossing, and then "it arrived" would say nothing about whether the first
    send worked. Send once, give delivery a real window, and only re-send if the
    send itself evidently never landed.
    """
    for attempt in range(sends):
        sent = type_once(from_base, from_sid, "Chat", "chat-compose", msg)
        for i in range(wait_each):
            time.sleep(1)
            if msg in (ex(to_base, to_sid, CHAT_TEXT) or ""):
                print(f"  {label} delivered: True (send {attempt + 1}, t={i + 1}s)")
                return True
        print(f"  {label} not delivered after send {attempt + 1} ({sent})")
    return False

def main():
    node_peer = sys.argv[1]
    print(f"MODE={MODE} ({'Direct/IDB — default arm' if MODE=='direct' else 'Worker/OPFS — opt-in arm'})")
    print(f"signaling node: {node_peer}\napp: {APP}  node_ws: {NODE_WS}  tag: {TAG!r}")
    print("NOTE: the URL carries NO webrtc params — provisioning comes from the registry.\n")

    sa = new_session(A_BASE); sb = new_session(B_BASE)
    print(f"A={sa}\nB={sb}")
    checks = {}
    try:
        if not (wait_boot(A_BASE, sa, "A") and wait_boot(B_BASE, sb, "B")): return 1

        # ── 1. cold: nothing provisioned, so nothing installed ────────────────
        print("\n── 1. cold boot, no connector ─────────────────")
        cold_a = log_has(A_BASE, sa, "no signaling node provisioned")
        cold_b = log_has(B_BASE, sb, "no signaling node provisioned")
        print(f"  A/B say 'no signaling node provisioned': {cold_a}/{cold_b}")
        checks["cold boot installs no establisher"] = cold_a and cold_b

        # ── 2. the user adds their connector ─────────────────────────────────
        print("\n── 2. add the connector through the Shell ─────")
        pa_ok = provision(A_BASE, sa, node_peer, "A")
        pb_ok = provision(B_BASE, sb, node_peer, "B")
        checks["the connector registry takes the node"] = pa_ok and pb_ok
        if not (pa_ok and pb_ok):
            print("\nRESULT: FAIL ❌ could not register the connector"); return 1

        # ── 3. reload: provisioning now comes from that choice ───────────────
        print("\n── 3. reload — provisioning from the registry ─")
        goto(A_BASE, sa); goto(B_BASE, sb)
        if not (wait_boot(A_BASE, sa, "A") and wait_boot(B_BASE, sb, "B")): return 1
        reg_a = log_has(A_BASE, sa, "provisioning from the selected connector")
        reg_b = log_has(B_BASE, sb, "provisioning from the selected connector")
        est_a = log_has(A_BASE, sa, "establisher")
        est_b = log_has(B_BASE, sb, "establisher")
        print(f"  A/B provisioned from their connector selection: {reg_a}/{reg_b}")
        print(f"  A/B installed a §6.5 establisher: {est_a}/{est_b}")
        checks["provisioning resolves from the durable registry"] = reg_a and reg_b
        checks["an establisher installs with no enable knob"] = est_a and est_b

        # Reflectors reached the ICE agent, when the rig is driving a NAT
        # topology. Asserted on the establisher-install line, which is where
        # provisioning meets the agent — a count read anywhere earlier would
        # prove the row was stored, not that the agent was configured. When
        # E2E_ICE is unset this asserts the LAN posture instead (0), so the
        # shared-bridge gates keep proving they add no third party.
        want_ice = expected_ice_urls()
        ice_a = [l for l in log_lines(A_BASE, sa) if "establisher" in l and "ice_urls" in l]
        ice_b = [l for l in log_lines(B_BASE, sb) if "establisher" in l and "ice_urls" in l]
        # Format-agnostic on purpose: the field reaches this log as
        # `ice_servers=0`, `"ice_servers":0`, or `\"ice_servers\":0` depending
        # on how the entry was serialized on its way to `__entity_browser_log`.
        # Matching the NUMBER rather than one spelling keeps the assertion about
        # the ICE agent instead of about the logger.
        ice_re = re.compile(r'ice_urls\D{0,4}(\d+)')
        def got(ls):
            vals = [int(m.group(1)) for l in ls for m in [ice_re.search(l)] if m]
            return bool(vals) and all(v == want_ice for v in vals)
        src = []
        if E2E_ICE:
            src.append(f"typed E2E_ICE={E2E_ICE}")
        if E2E_NODE_REFLECTION:
            src.append(f"node-advertised §4.5.1={E2E_NODE_REFLECTION}")
        print(f"  A/B establisher ice_urls == {want_ice}: {got(ice_a)}/{got(ice_b)}"
              + (f"   ({' + '.join(src)})" if src else "   (host-only, no reflector)"))
        if not ice_a or not ice_b:
            print("  !! no establisher line carried an ice_urls field — cannot classify")
        checks[f"the ICE agent is configured with {want_ice} reflector url(s)"] = (
            bool(ice_a) and bool(ice_b) and got(ice_a) and got(ice_b)
        )

        open_shell(A_BASE, sa, "A"); open_shell(B_BASE, sb, "B")
        pa = bound_peer_id(A_BASE, sa, "A")
        pb = bound_peer_id(B_BASE, sb, "B")
        print(f"  peer A = {pa}\n  peer B = {pb}")
        if not (pa and pb and pa != pb):
            print("\nRESULT: FAIL ❌ distinct peer ids required"); return 1

        # The registry must have survived the reload for `meet` to find a node.
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            if not until_listed(base, sid, "connector ls", "●", tries=3):
                print(f"  {lbl} connector selection did not survive the reload — re-adding")
                provision(base, sid, node_peer, lbl)

        # ── 4. both meet at the same name ────────────────────────────────────
        print(f"\n── 4. both `meet tag {TAG}` ───────────────────")
        # A bucket holds its messages for the 60s TTL, so the two searches only
        # have to overlap — they need not start together.
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            started, _ = run_until(base, sid, f"meet tag {TAG}",
                                   shell_says(base, sid, "meeting at"), tries=10)
            print(f"  {lbl} meet started: {started}")

        learned_a, learned_b = [], []
        for i in range(45):
            time.sleep(1)
            learned_a, learned_b = met_ids(A_BASE, sa), met_ids(B_BASE, sb)
            if pb in learned_a and pa in learned_b:
                print(f"  both sides learned the other's id at t={i+1}s"); break
            if i % 10 == 9:
                print(f"  t={i+1}s  A learned {len(learned_a)}, B learned {len(learned_b)}")

        print(f"  A learned: {learned_a}\n  B learned: {learned_b}")
        # The claim is not "a meet reported something" — it is that what it
        # reported is the counterpart's real id.
        checks["A met B's actual peer id"] = pb in learned_a
        checks["B met A's actual peer id"] = pa in learned_b
        if not (pb in learned_a and pa in learned_b):
            for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
                print(f"\n[{lbl}] shell scrollback:\n{ex(base, sid, SHELL_TEXT)}")
            print("\nRESULT: FAIL ❌ the meet did not introduce the two peers")
            return 1

        # ── 5. chat over the discovered id ───────────────────────────────────
        print("\n── 5. chat, bound to the DISCOVERED id ────────")
        chat_t0 = time.time()
        ex(A_BASE, sa, spawn_script("Chat")); ex(B_BASE, sb, spawn_script("Chat"))
        time.sleep(1.5)
        # Each side binds the id its OWN meet handed it — nothing passed in.
        # Confirmed by the start-by-id input DISAPPEARING: the picker renders
        # only while unbound (`render_start_by_id`), so its absence is the app
        # saying the conversation took. Asserting that the messages element
        # merely exists would pass before any bind happened.
        for base, sid, lbl, pid in ((A_BASE, sa, "A", pb), (B_BASE, sb, "B", pa)):
            bound, n = type_until(base, sid, "Chat", "chat-start-peer", pid,
                                  lambda b=base, s=sid: find_field(b, s, "Chat",
                                                                   "chat-start-peer") is None,
                                  tries=10)
            print(f"  {lbl} bound to the id it learned: {bound} (after {n} attempt(s))")
            if not bound:
                checks["chat binds the discovered id"] = False

        for i in range(30):
            time.sleep(1)
            if log_has(A_BASE, sa, "data channel is OPEN") and \
               log_has(B_BASE, sb, "data channel is OPEN"):
                print(f"  both channels open at t={i+1}s"); break

        msg_a, msg_b = "hello from A, met at a name", "reply from B, met at a name"
        got_b = send_and_wait(A_BASE, sa, B_BASE, sb, msg_a, "A->B")
        got_a = send_and_wait(B_BASE, sb, A_BASE, sa, msg_b, "B->A")
        checks["A->B message delivered"] = got_b
        checks["B->A message delivered"] = got_a

        # ── 6. the header agrees with reality ────────────────────────────────
        # Messages just crossed in both directions, so the link is live by
        # demonstration — the header must say so. This is the only place that
        # proves the Chat reachability surface on the WORKER arm against a REAL
        # kernel `system/peer/status`: the read goes through the worker cache
        # mirror, which is fed only for subscribed prefixes, so a Chat window
        # that forgot to watch `system/peer/status` reads Unknown forever and
        # paints a dash next to a conversation that is visibly working.
        #
        # Two assertions, because the reason line has a relevance half. It must
        # be ABSENT here: we hold no establisher claim either way, and a note
        # saying "nothing can connect to you" beside a working chat is the
        # permanent-warning shape users learn to ignore.
        print("\n── 6. the reachability header ─────────────────")
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            reach = ""
            for _ in range(20):
                reach = ex(base, sid, REACH_TEXT) or ""
                if "Connected" in reach:
                    break
                time.sleep(1)
            print(f"  {lbl} header: {reach!r}")
            checks[f"{lbl} header reads Connected"] = "Connected" in reach
            checks[f"{lbl} header raises no false unreachable note"] = (
                "can’t be reached back" not in reach
            )

        # The establishment diagnostic, printed on EVERY run rather than only in
        # the negative control. `WebRtcError::Timeout` carries the two facts a
        # bare failure cannot — did we offer/answer, and did trickled candidates
        # reach the peer connection (posted/fed) — and without printing it a
        # failed traversal is indistinguishable from a failed rendezvous. Cost
        # is two log reads; the run that motivated this had all of it in the
        # browser and none of it on screen.
        print("\n── §6.5 establishment (last failure per side) ─")
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            fails = [l for l in log_lines(base, sid) if "negotiation to" in l and "failed" in l]
            if not fails:
                print(f"  {lbl}: no negotiation failures logged")
                continue
            # FIRST and last, because they answer different questions and the
            # last one lies about the first. Once a bucket fills or a retry
            # storm sets in, every later failure reports the consequence; only
            # the first negotiation ran against a clean rendezvous, so it is the
            # one that says whether a path existed. (Learned here: the last line
            # read `429 bucket_full` on a run whose real question was ICE.)
            print(f"  {lbl}: {len(fails)} failed")
            print(f"     first: {fails[0][:400]}")
            if len(fails) > 1:
                print(f"     last:  {fails[-1][:400]}")
            # And the distinct failure classes, so a mixed run is not read as
            # whichever class happened to land last.
            classes = {}
            for f in fails:
                for k in ("bucket_full", "Timeout", "no live path", "refused", "verification"):
                    if k in f:
                        classes[k] = classes.get(k, 0) + 1
            if classes:
                print("     classes: " + ", ".join(f"{k}={v}" for k, v in sorted(classes.items())))

        print("\n── meet-then-chat gate ───────────────────────")
        for k, v in checks.items():
            print(f"   {'✅' if v else '❌'}  {k}")

        # ── 7. WHICH retry shape? (EXTENSION-SIGNALING §13 item 6) ───────────
        # The node sees ~970 offer deposits per side against an unreachable
        # pair and cannot tell two very different causes apart, because deposits
        # key on (caller, rendezvous_key) and every retry reuses the key:
        #
        #   (a) ONE establishment retrying without bound, or
        #   (b) MANY bounded establishments, each re-triggered by a caller.
        #
        # From here it is directly observable. Each `establish_live` that fails
        # emits exactly one "negotiation to '<peer>' failed" console line
        # (main_thread_establish.rs), so counting those lines counts COMPLETED
        # negotiations. Deposits ÷ negotiations is then the per-negotiation
        # deposit count — the number §11.5 bounds.
        if EXPECT_NO_MEDIA:
            elapsed = max(time.time() - chat_t0, 1e-9)
            print("\n── 7. retry shape (§13 item 6) ───────────────")
            print(f"  chat window open for {elapsed:.0f}s")
            for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
                lines = log_lines(base, sid)
                failed = [l for l in lines if "negotiation to" in l and "failed" in l]
                # Tell the two failure classes apart: a policy refusal (mixed
                # build) is NOT a NAT result and would invalidate the reading.
                policy = [l for l in failed if "policy" in l]
                rate = len(failed) / elapsed
                print(f"  {lbl}: NEGOTIATION_ATTEMPTS={len(failed)} policy_refusals={len(policy)} "
                      f"rate={rate:.1f}/s")
                if policy:
                    print(f"  {lbl}: !! policy refusals present — mixed build, not a NAT result")


            # NEGATIVE CONTROL (the split-network rig). Inverted on purpose, and
            # only for the media half: rendezvous rides the signaling node, which
            # both peers reach through the host, so it MUST still work. The data
            # channel is peer-to-peer and this session offers host candidates
            # only (`resolve_webrtc_provisioning` hardcodes `ice_servers:
            # Vec::new()`), so across two isolated networks it MUST NOT open.
            #
            # Passing here is therefore evidence of a REAL LIMIT, not of health.
            # If it ever starts delivering, that is the interesting result and
            # this gate says so loudly rather than going quietly green: either
            # the isolation leaked (check the rig's own A↔B probe) or we gained
            # traversal (then flip EXPECT_NO_MEDIA off — this rig becomes the
            # positive NAT gate it was always meant to grow into).
            rendezvous = [k for k in checks if "met" in k or "connector" in k
                          or "provisioning" in k or "establisher" in k]
            delivery = [k for k in checks if "delivered" in k]
            rv_ok = all(checks[k] for k in rendezvous)
            delivered = any(checks[k] for k in delivery)
            print("\n── negative control: two isolated networks ───")
            print(f"   rendezvous over the node (must hold) : {'✅' if rv_ok else '❌'}")
            print(f"   media delivered (must NOT)           : {'❌ delivered' if delivered else '✅ no path'}")
            ok = rv_ok and not delivered
            if delivered:
                print("\nRESULT: FAIL ❌ media crossed two isolated networks.")
                print("  Either the isolation leaked or traversal now works.")
                print("  Check the rig's A->B probe before believing the latter.")
            elif not rv_ok:
                print("\nRESULT: FAIL ❌ rendezvous broke — the rig lost the node,")
                print("  so this run says nothing about media reachability.")
            else:
                print("\nRESULT: PASS ✅ discovery works, media does not —")
                print("  host candidates cannot cross NATs. This is the ICE gap,")
                print("  measured (EXTENSION-SIGNALING §13 item 5).")
            return 0 if ok else 1

        ok = all(checks.values())
        print(f"\nRESULT: {'PASS ✅ name -> id -> connection -> message' if ok else 'FAIL ❌'}")
        return 0 if ok else 1
    finally:
        rq(A_BASE, "DELETE", f"/session/{sa}"); rq(B_BASE, "DELETE", f"/session/{sb}")

if __name__ == "__main__":
    sys.exit(main())

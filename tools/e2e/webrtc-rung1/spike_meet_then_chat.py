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
import json, os, re, subprocess, sys, time, urllib.request

MODE = os.environ.get("MODE", "direct").strip().lower()
# Skip step 3's reload and require the seam to arm in-session instead — the
# regression gate for `src/late_establish.rs`. See step 3.
NO_RELOAD = os.environ.get("NO_RELOAD", "").strip() in ("1", "true", "yes")
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

# ── the `survives idle` phase (EXTENSION-NETWORK Amendment 14) ───────────────
# Amendment 14 puts a MUST on the connection the §10.3 seam returns: it "MUST run
# keepalive (§5), because a punched NAT mapping expires on silence and an idle
# punched connection dies in a way no same-host test reproduces." §11.5's gate
# says the same in one phrase — "a direct punched transport that SURVIVES IDLE".
#
# IDLE_SECS>0 turns the phase on; it is off by default because it costs its own
# duration and the traversal gate does not need it. It is only meaningful on
# TOPOLOGY=nat with a conntrack UDP timeout BELOW the quiet window — otherwise
# the mapping was never at risk and a green result means nothing. That is
# asserted, not documented: see `idle_preconditions`.
IDLE_SECS = int(os.environ.get("IDLE_SECS", "0") or 0)
TOPOLOGY = os.environ.get("TOPOLOGY", "shared").strip().lower()
# What the routers were told, so the run can check the mapping was really at risk
# (nat_topology.sh applies it; it is echoed here rather than re-read per router).
UDP_TIMEOUT = int(os.environ.get("UDP_TIMEOUT", "180") or 180)

A_BASE, B_BASE = "http://localhost:4446", "http://localhost:4447"
APP = "http://host.containers.internal:8092"
NODE_WS = "ws://host.containers.internal:4071"

def rq(base, method, path, body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)

# **mDNS host-candidate obfuscation — off by default here, and that is a RIG
# accommodation, not a statement about browsers.**
#
# Chrome and Firefox both ship this ON: a host candidate's IP is replaced by a
# random `{uuid}.local` name resolved over multicast DNS at connection time. On a
# real LAN that resolves and two peers connect on host candidates alone — no
# STUN, no TURN. Between podman containers it has not been shown to resolve,
# which is why every spike in this directory turns it off.
#
# The cost of that accommodation is that **the shipped candidate path on the most
# common topology (two people on one Wi-Fi) has no coverage** — our green gates
# all run raw-IP host candidates, which is not what a user's browser sends.
#
# `E2E_MDNS=1` runs it the way a real browser does. Treat a failure under it as a
# question about the RIG first (does multicast cross this bridge at all?) and
# about the app second — a red result here is only evidence about browsers if
# multicast is known to work between the containers.
_MDNS = os.environ.get("E2E_MDNS", "").strip() not in ("", "0")
_PREFS = {"media.peerconnection.ice.obfuscate_host_addresses": _MDNS}
if MODE == "worker":
    _PREFS["dom.securecontext.allowlist"] = "host.containers.internal"
    _PREFS["dom.securecontext.whitelist"] = "host.containers.internal"

# ── which ENGINE each side runs, and why that is a per-side knob ─────────────
#
# **Every WebRTC gate in this repo was Firefox↔Firefox until this existed**, and
# that is not a neutral convenience — the two engines disagree about the data
# channel's `maxMessageSize` by four orders of magnitude (Firefox advertises
# ~1 GiB and fragments internally; Chromium advertises 262 144 and does not, and
# a pair takes the smaller of the two). A same-engine Firefox rig is the one
# population in which an oversized `send()` cannot happen, so a size defect on
# the transport was not merely untested here, it was **unreachable** — the gates
# were green because the failing configuration was excluded. Reported from a real
# Android(Chrome) → desktop(Firefox) transfer that stalled after one progress
# line while `make e2e-webrtc-file` stayed green.
#
# Per-side rather than one switch because the interesting configuration is the
# MIXED one: the ceiling is negotiated between two engines, so Chrome↔Chrome
# would agree on 256 KiB and Firefox↔Firefox on ~1 GiB, and neither exercises the
# asymmetry a real pair has. `rung1_repro.sh` reads the same two variables to
# pick each container's image; they must agree, which is why the spike prints
# what it actually got from the browser rather than what it was told.
ENGINE_A = (os.environ.get("ENGINE_A", "firefox").strip().lower() or "firefox")
ENGINE_B = (os.environ.get("ENGINE_B", "firefox").strip().lower() or "firefox")


def _firefox_caps():
    return {
        "browserName": "firefox",
        # `prefs` is copied per side: `spike_file_over_webrtc` mutates the
        # download prefs, and a shared dict would leak one side's edits into the
        # other's session.
        "moz:firefoxOptions": {"args": ["-headless"], "prefs": dict(_PREFS)},
    }


def _chrome_caps():
    # `--headless=new` is the real renderer rather than the legacy shell, which
    # matters here: the legacy headless mode has its own WebRTC quirks and would
    # make a red result unattributable.
    args = ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
            "--disable-gpu"]
    if not _MDNS:
        # The Firefox side turns obfuscation off via a pref; Chromium's switch is
        # a feature flag. Both must agree or the two sides send different
        # candidate shapes and the rig stops being one topology.
        args.append("--disable-features=WebRtcHideLocalIpsWithMdns")
    if MODE == "worker":
        # The Chromium counterpart of `dom.securecontext.allowlist`: OPFS gates
        # on a secure context, and the rig serves plain HTTP.
        args.append(f"--unsafely-treat-insecure-origin-as-secure={APP}")
        args.append("--user-data-dir=/tmp/entity-chrome-profile")
    return {
        "browserName": "chrome",
        "goog:chromeOptions": {
            "args": args,
            "prefs": {
                "download.default_directory": "/tmp",
                "download.prompt_for_download": False,
            },
        },
    }


def caps_for(base):
    """W3C capabilities for whichever side `base` addresses."""
    engine = ENGINE_A if base == A_BASE else ENGINE_B
    if engine == "chrome":
        return {"capabilities": {"alwaysMatch": _chrome_caps()}}
    if engine != "firefox":
        raise SystemExit(f"unknown engine {engine!r} — expected 'firefox' or 'chrome'")
    return {"capabilities": {"alwaysMatch": _firefox_caps()}}


# Retained as the Firefox default so the spikes that reach for `meet.CAPS`
# directly keep working. `caps_for` is what `new_session` uses.
CAPS = {"capabilities": {"alwaysMatch": _firefox_caps()}}

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
# Distinctive fragments of every sentence `crate::reachability` can add to the
# Chat header. None of them may appear beside a connection that is demonstrably
# working — a diagnosis raised during ordinary establishment, or left over after
# it succeeded, is worse than silence, which is what the four "raises no false
# unreachable note" assertions have always been about.
FALSE_NOTES = (
    "No reflector is set up",
    "reflector didn’t answer",
    "needs a relay",
)
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
    reply = rq(base, "POST", "/session", caps_for(base))["value"]
    sid = reply["sessionId"]
    # Print what the grid actually started, not what we asked for. A mixed-engine
    # run whose second container silently came up Firefox is a rig that reports a
    # cross-engine pass it never ran — the exact shape of failure this whole
    # per-side knob exists to end.
    caps = reply.get("capabilities", {})
    print(f"  {'A' if base == A_BASE else 'B'} engine: "
          f"{caps.get('browserName', '?')} {caps.get('browserVersion', '?')}")
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

def wait_log(base, sid, needle, secs):
    """Poll for a log line rather than sleeping a fixed amount.

    The late arm happens on a frame, not on a request, so how long it takes is a
    function of when the connector write lands and when the next frame runs.
    A fixed sleep here would be either flaky or slow, and the value it would
    have to be is exactly the thing under test.
    """
    deadline = time.time() + secs
    while time.time() < deadline:
        if log_has(base, sid, needle):
            return True
        time.sleep(0.5)
    return False

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

# A RELAY (TURN) for the connector row, with its credentials. Empty by default,
# so every existing gate reproduces exactly as before.
#
# Proves the shipped path end to end — typed into the Shell's `connector add`,
# stored in the durable row, resolved through provisioning, landed on the ICE
# agent — WITHOUT needing a real TURN server, which is the same thing the
# reflector assertion does for `stun:`. What it deliberately does NOT prove is
# that relayed media flows; that needs a relay to point at.
E2E_RELAY = os.environ.get("E2E_RELAY", "").strip()
E2E_RELAY_USER = os.environ.get("E2E_RELAY_USER", "").strip()
E2E_RELAY_CRED = os.environ.get("E2E_RELAY_CRED", "").strip()

def expected_ice_servers():
    """How many `RTCIceServer` ENTRIES the agent should hold.

    Counted in entries, not urls, because that is the property a relay has and
    a reflector does not: `parse_ice_urls` packs every reflector into ONE entry,
    while a relay is always a SECOND one — an `RTCIceServer` carries a single
    credential pair for all its urls, so a credentialed relay cannot share an
    entry with credential-free reflectors without either leaking the username
    onto them or losing it from the relay.

    So `ice_urls` sees the §4.5.1 merge and `ice_servers` sees the relay split;
    each assertion needs its own counter, and using one for both would be blind
    to exactly the thing it was added for.
    """
    return (1 if expected_ice_urls() else 0) + (1 if E2E_RELAY else 0)

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

def expected_ice_urls_total():
    """`ice_urls` as the app logs it — every url across EVERY entry.

    The relay's urls land in the same sum, so this is the reflector merge plus
    the relay. Kept separate from `expected_ice_urls` so the §4.5.1 dedup/union
    rule above stays readable as itself: that function answers "what should the
    merge produce", this one answers "what does the log field count".
    """
    return expected_ice_urls() + len(
        [u for u in re.split(r"[,\s]+", E2E_RELAY) if u] if E2E_RELAY else []
    )

def provision(base, sid, node_peer, label):
    """Add + select the connector through the Shell — the user's own surface."""
    open_shell(base, sid, label)
    # `ice=` rides the SAME `connector add` a user types. Deliberately not
    # injected by URL: the durable registry row is the shipped path, and it is
    # the one that has to carry reflectors all the way to the ICE agent.
    ice_arg = f" ice={E2E_ICE}" if E2E_ICE else ""
    # Same surface, same reason: a relay that only works when injected by URL
    # would prove nothing about the field a person actually fills in.
    relay_arg = ""
    if E2E_RELAY:
        relay_arg = (f" relay={E2E_RELAY} relay_user={E2E_RELAY_USER}"
                     f" relay_cred={E2E_RELAY_CRED}")
    ok_add = until_listed(base, sid,
                          f"connector add {node_peer} {NODE_WS}{ice_arg}{relay_arg} rung1",
                          short_pid(node_peer))
    # `connector ls` marks the selection with ●.
    ok_use = until_listed(base, sid, f"connector use {node_peer}", "●")
    print(f"  {label} connector added: {ok_add}   selected: {ok_use}")
    return ok_add and ok_use

# `met <short>  <full-id>` is what pump_meet pushes per discovered peer.
MET_RE = re.compile(r"met\s+\S+\s+([1-9A-HJ-NP-Za-km-z]{40,})")

def _router(name, *args):
    """Run an iptables command in one of the rig's routers. Returns stdout, or
    None if the router is absent or the command failed — a measurement that
    cannot be taken must read as ABSENT, never as zero. A silent 0 here would
    be indistinguishable from "the link went completely quiet", which is one of
    the answers this phase exists to report."""
    try:
        out = subprocess.run(["podman", "exec", name, *args],
                             capture_output=True, text=True, timeout=20)
    except Exception:
        return None
    return out.stdout if out.returncode == 0 else None


def idle_counters_arm():
    """Install a UDP packet counter on each router's FORWARD path and zero it.

    A user chain used purely as a counter: the jump is non-terminating (an empty
    chain returns and traversal continues), so this observes the path without
    changing it — the DROP rules below still see every packet they saw before.
    Read back by CHAIN NAME, never by rule position: position moves the moment
    anything else is inserted, and a mis-parsed row would be reported as a
    cadence, which is the one number this phase turns on.

    Deliberately counts ALL forwarded UDP rather than splitting by direction.
    Splitting needs the router's LAN subnet, and deriving it from `ip route`
    picks between two link routes — one of which is the transit side. A wrong
    subnet yields a plausible small number instead of an error, and the whole
    point here is that an unmeasurable mechanism must read as ABSENT."""
    for s in ("a", "b"):
        rtr = f"rtc-router-{s}"
        _router(rtr, "iptables", "-N", "IDLEUDP")
        _router(rtr, "iptables", "-F", "IDLEUDP")
        # -C first so a re-run cannot stack duplicate counters on one path.
        if _router(rtr, "iptables", "-C", "FORWARD", "-p", "udp",
                   "-j", "IDLEUDP") is None:
            if _router(rtr, "iptables", "-I", "FORWARD", "1", "-p", "udp",
                       "-j", "IDLEUDP") is None:
                return None
        if _router(rtr, "iptables", "-Z", "FORWARD") is None:
            return None
    return True


def idle_counters_read():
    """{router: packets forwarded as UDP} — or None if unmeasurable."""
    res = {}
    for s in ("a", "b"):
        rtr = f"rtc-router-{s}"
        txt = _router(rtr, "iptables", "-L", "FORWARD", "-v", "-x", "-n")
        if txt is None:
            return None
        hit = None
        for line in txt.splitlines():
            f = line.split()
            if len(f) >= 3 and f[2] == "IDLEUDP":
                try:
                    hit = int(f[0])
                except ValueError:
                    return None
        if hit is None:
            return None
        res[s] = hit
    return res


def idle_preconditions():
    """Why this run is or is not allowed to claim anything about idle survival.

    Returns (ok, reason). The mapping must have been genuinely at risk: a quiet
    window shorter than the NAT's UDP timeout tests nothing, and would report a
    confident green for a peer that runs no keepalive at all — §11.5.1's whole
    warning, one substrate along."""
    if TOPOLOGY != "nat":
        return False, (f"TOPOLOGY={TOPOLOGY} — there is no NAT in path, so no "
                       "mapping can expire and idle proves nothing (§11.5.1)")
    if IDLE_SECS <= UDP_TIMEOUT:
        return False, (f"quiet window {IDLE_SECS}s <= conntrack UDP timeout "
                       f"{UDP_TIMEOUT}s — the mapping was never at risk; raise "
                       "IDLE_SECS or lower UDP_TIMEOUT")
    return True, (f"quiet window {IDLE_SECS}s vs conntrack UDP timeout "
                  f"{UDP_TIMEOUT}s ({IDLE_SECS / UDP_TIMEOUT:.1f}x)")


def channel_opens(base, sid):
    """How many times a data channel has opened. A re-establishment during the
    quiet window would deliver the post-idle message just fine — and would mean
    the transport did NOT survive idle, it was rebuilt. This is the difference
    the gate turns on, and delivery alone cannot see it."""
    return sum(1 for l in log_lines(base, sid) if "data channel is OPEN" in l)


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
    # Kept apart from `checks` on purpose — these decide whether the idle result
    # MEANS anything, not whether it passed. Mixing them makes a run that could
    # not test idle survival indistinguishable from one where it broke.
    idle_inconclusive = {}
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

        # ── 3. the establisher arrives — with or WITHOUT a reload ────────────
        #
        # `NO_RELOAD=1` is the regression gate for late arming
        # (`src/late_establish.rs`). Until 2026-09-07 the seam was a constructor
        # argument, so a node chosen mid-session could not reach the running
        # peer and this step HAD to reload; a fresh profile therefore spent its
        # whole first session findable and unreachable, which reached the
        # operator as "they detect each other but chat doesn't work". The slot
        # is now installed empty at boot and filled on the next frame.
        #
        # Direct arm only, and that is not a gap being papered over: the Worker
        # arm takes its provisioning from `InitParams`, which is Init-only
        # upstream, so there the reload is still genuinely required. The gate
        # refuses rather than silently passing a run that proves nothing.
        if NO_RELOAD and MODE != "direct":
            print("\nRESULT: FAIL ❌ NO_RELOAD is a Direct-arm claim; "
                  f"MODE={MODE} still needs the reload (InitParams is Init-only)")
            return 1

        if NO_RELOAD:
            print("\n── 3. NO reload — the seam is armed in-session ─")
            armed_a = wait_log(A_BASE, sa, "armed the §6.5 establisher", 20)
            armed_b = wait_log(B_BASE, sb, "armed the §6.5 establisher", 20)
            print(f"  A/B logged an in-session arm: {armed_a}/{armed_b}")
            # **A step indicator, not the claim.** It says the arming path ran;
            # it cannot say the armed slot is the one wired into the peer.
            # Measured: a neuter that installed the seam only when boot already
            # had a node left this True — the detached slot is still armed — and
            # the run failed two steps later. So the assertions that
            # DISCRIMINATE are message delivery and the unreachable-note checks
            # below, and this one exists to say which step broke when they do.
            checks["the in-session arming path runs"] = armed_a and armed_b
            if not (armed_a and armed_b):
                print("\nRESULT: FAIL ❌ THE SEAM DID NOT ARM IN-SESSION. A rendezvous "
                      "node was selected through the Shell and the running peer never "
                      "picked it up, so this session can find peers and can never be "
                      "connected back to. That is the 2026-09-07 report: discovery "
                      "works, chat does not, and the only escape is a reload nobody "
                      "is told to do.")
                return 1
        else:
            print("\n── 3. reload — provisioning from the registry ─")
            goto(A_BASE, sa); goto(B_BASE, sb)
            if not (wait_boot(A_BASE, sa, "A") and wait_boot(B_BASE, sb, "B")): return 1
            reg_a = log_has(A_BASE, sa, "provisioning from the selected connector")
            reg_b = log_has(B_BASE, sb, "provisioning from the selected connector")
            print(f"  A/B provisioned from their connector selection: {reg_a}/{reg_b}")
            checks["provisioning resolves from the durable registry"] = reg_a and reg_b

        est_a = log_has(A_BASE, sa, "establisher")
        est_b = log_has(B_BASE, sb, "establisher")
        print(f"  A/B installed a §6.5 establisher: {est_a}/{est_b}")
        checks["an establisher installs with no enable knob"] = est_a and est_b

        # Reflectors reached the ICE agent, when the rig is driving a NAT
        # topology. Asserted on the establisher-install line, which is where
        # provisioning meets the agent — a count read anywhere earlier would
        # prove the row was stored, not that the agent was configured. When
        # E2E_ICE is unset this asserts the LAN posture instead (0), so the
        # shared-bridge gates keep proving they add no third party.
        want_ice = expected_ice_urls_total()
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
        checks[f"the ICE agent is configured with {want_ice} ICE url(s)"] = (
            bool(ice_a) and bool(ice_b) and got(ice_a) and got(ice_b)
        )

        # The RELAY reached the agent as its own entry. Asserted on the same
        # install line for the same reason as the reflectors — anywhere earlier
        # would prove the row was stored, not that the agent was configured.
        want_servers = expected_ice_servers()
        srv_re = re.compile(r'ice_servers\D{0,4}(\d+)')
        def got_servers(ls):
            vals = [int(m.group(1)) for l in ls for m in [srv_re.search(l)] if m]
            return bool(vals) and all(v == want_servers for v in vals)
        print(f"  A/B establisher ice_servers == {want_servers}: "
              f"{got_servers(ice_a)}/{got_servers(ice_b)}"
              + (f"   (relay {E2E_RELAY} as user '{E2E_RELAY_USER}')" if E2E_RELAY else ""))
        checks[f"the ICE agent holds {want_servers} ICE server entry/entries"] = (
            bool(ice_a) and bool(ice_b) and got_servers(ice_a) and got_servers(ice_b)
        )

        # ── mDNS: prove the PREF IS IN EFFECT, or a green run proves nothing ──
        # Under `E2E_MDNS=1` the whole point is that host candidates carry
        # `{uuid}.local` names instead of raw IPs — what a real browser sends.
        # But a PASS on its own cannot distinguish "mDNS resolved and the peers
        # connected" from "the pref never applied and this was the ordinary
        # raw-IP run". Firefox scopes obfuscation by permission state, so it is
        # genuinely possible for the pref to be set and not bite.
        #
        # So gather candidates from a throwaway `RTCPeerConnection` in the page
        # and read the SDP. Printed on PASS as well as FAIL — a green run has to
        # carry its own evidence, or the next seat re-derives this.
        if _MDNS:
            probe = (
                "const done = arguments[arguments.length-1];"
                "const pc = new RTCPeerConnection({iceServers:[]});"
                "pc.createDataChannel('probe');"
                "pc.onicegatheringstatechange = () => {"
                "  if (pc.iceGatheringState === 'complete') {"
                "    const sdp = (pc.localDescription && pc.localDescription.sdp) || '';"
                "    pc.close(); done(sdp); } };"
                "pc.createOffer().then(o => pc.setLocalDescription(o));"
                "setTimeout(() => { const sdp = (pc.localDescription && pc.localDescription.sdp) || '';"
                "  try { pc.close(); } catch(e) {} done(sdp); }, 5000);"
            )
            def cands(base, sid):
                sdp = rq(base, "POST", f"/session/{sid}/execute/async",
                         {"script": probe, "args": []})["value"] or ""
                return [l.strip() for l in sdp.splitlines() if l.startswith("a=candidate")]
            ca, cb = cands(A_BASE, sa), cands(B_BASE, sb)
            mdns_a = any(".local" in c for c in ca)
            mdns_b = any(".local" in c for c in cb)
            print(f"  mDNS host candidates (E2E_MDNS=1): A={mdns_a} B={mdns_b}")
            for lbl, cs in (("A", ca), ("B", cb)):
                for c in cs[:3]:
                    print(f"     {lbl}: {c[:120]}")
            checks["host candidates are obfuscated to .local (what a real browser sends)"] = (
                mdns_a and mdns_b
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
            if EXPECT_NO_MEDIA:
                # THE SPLIT RIG IS THE `NoReflector` TOPOLOGY, exactly: host
                # candidates only (`ice_servers: Vec::new()`), two isolated
                # networks, so every negotiation genuinely fails and the
                # classifier has something TRUE to say. This is the positive
                # half of the diagnosis feature and the only gate that proves it
                # can produce a note at all — everywhere else the requirement is
                # that it stays quiet, which a classifier wired to nothing also
                # satisfies.
                #
                # It reads the RENDERED TEXT, deliberately. The classifier has
                # native tests; an exit code cannot tell "classified correctly"
                # from "classified correctly and rendered nowhere" (AP25).
                named = any(n in reach for n in FALSE_NOTES)
                checks[f"{lbl} header names WHY it is unreachable"] = named
                # And specifically the right one — "no reflector configured",
                # not "this network needs a relay". Recommending a relay to
                # someone who has not configured a reflector sends them to buy
                # the wrong thing, and both notes appear in exactly the
                # situations that look alike from outside.
                checks[f"{lbl} names the reflector, not a relay"] = (
                    "No reflector is set up" in reach and "needs a relay" not in reach
                )
            else:
                checks[f"{lbl} header raises no false unreachable note"] = (
                    "can’t be reached back" not in reach
                    # The classifier's notes belong to the same rule and are
                    # checked here rather than in a gate of their own, because
                    # this is the case that matters: messages just crossed in
                    # both directions, so any of these sentences is provably
                    # false. If the strings are reworded, update `FALSE_NOTES` —
                    # a check that silently stops matching is the failure mode.
                    and not any(n in reach for n in FALSE_NOTES)
                )

        # ── 7. survives idle — EXTENSION-NETWORK Amdt 14 / §11.5 ─────────────
        # Establishment and carriage are the easy halves. The gate's own phrase
        # is "a direct punched transport that SURVIVES IDLE", and a punched
        # mapping dies on silence — so the only way to test it is to actually be
        # silent for longer than the mapping lives, then speak again.
        #
        # Three things are measured, and the third is the one that keeps this
        # honest. (a) does a message cross after the quiet window; (b) did the
        # channel stay up or was it rebuilt (delivery cannot tell you which);
        # (c) WHAT CROSSED THE ROUTER while the app was "idle" — because if the
        # application itself chatters through the quiet window, the mapping was
        # refreshed by traffic and the run has tested nothing. Arch asked for
        # exactly this line: record which mechanism held the mapping open.
        if IDLE_SECS:
            print(f"\n── 7. survives idle ({IDLE_SECS}s quiet) ──────────")
            ok_pre, why = idle_preconditions()
            print(f"  precondition: {'✓' if ok_pre else '✗'} {why}")

            opens_before = {l: channel_opens(b, s)
                            for b, s, l in ((A_BASE, sa, "A"), (B_BASE, sb, "B"))}
            armed = idle_counters_arm()
            t0 = time.time()
            # Sleep in chunks purely so a long quiet window reports progress; no
            # WebDriver call is made inside it, because every `execute` pumps the
            # page and this window must be as quiet as the app can be.
            while time.time() - t0 < IDLE_SECS:
                left = IDLE_SECS - (time.time() - t0)
                time.sleep(min(15, max(0.5, left)))
                print(f"     quiet … {int(time.time() - t0)}s/{IDLE_SECS}s", flush=True)
            elapsed = time.time() - t0

            counts = idle_counters_read() if armed else None
            mechanism = "UNMEASURED — router counters unavailable"
            app_chatty = False
            if counts:
                tot = sum(counts.values())
                rate = tot / elapsed if elapsed else 0.0
                per_side = {s: n / elapsed for s, n in counts.items()}
                print("  UDP packets forwarded during the quiet window: " +
                      ", ".join(f"router-{s}={counts[s]}" for s in sorted(counts)) +
                      f"  ({rate:.2f} pkt/s total, "
                      + ", ".join(f"{s}={per_side[s]:.2f}/s" for s in sorted(counts))
                      + ")")
                # Classification, with the numbers that separate the cases. ICE
                # consent freshness (RFC 7675) re-checks the selected pair on a
                # low single-digit-second period, so it lands near 0.2-0.5 pkt/s
                # per direction and nowhere near an application poll.
                if rate < 0.02:
                    mechanism = ("NOTHING — no packet crossed. If the message "
                                 "below still lands, the mapping outlived the "
                                 "quiet window on its own or was rebuilt.")
                elif max(per_side.values()) > 2.0:
                    mechanism = (f"APPLICATION TRAFFIC ({rate:.1f} pkt/s) — the "
                                 "app was not quiet, so this run does NOT test "
                                 "idle survival")
                    app_chatty = True
                else:
                    mechanism = (f"PERIODIC KEEPALIVE at {rate:.2f} pkt/s "
                                 f"(~{1/max(rate,1e-9):.1f}s period) — consistent "
                                 "with the browser's own ICE consent freshness "
                                 "(RFC 7675), NOT with an application-tier §5 "
                                 "keepalive, which this peer does not run on the "
                                 "WebRTC path")
            print(f"  mechanism that held the mapping open: {mechanism}")

            msg_a2 = "after the quiet window, from A"
            msg_b2 = "after the quiet window, from B"
            got_b2 = send_and_wait(A_BASE, sa, B_BASE, sb, msg_a2, "A->B post-idle")
            got_a2 = send_and_wait(B_BASE, sb, A_BASE, sa, msg_b2, "B->A post-idle")
            opens_after = {l: channel_opens(b, s)
                           for b, s, l in ((A_BASE, sa, "A"), (B_BASE, sb, "B"))}
            rebuilt = {l: opens_after[l] - opens_before[l] for l in opens_before}
            print(f"  data-channel opens during idle: {rebuilt} "
                  f"(0 = survived; >0 = re-established, which is NOT surviving)")

            checks["post-idle A->B delivered"] = got_b2
            checks["post-idle B->A delivered"] = got_a2
            checks["the channel survived rather than re-establishing"] = \
                all(v == 0 for v in rebuilt.values())
            # A run that cannot claim anything must NOT report PASS — a vacuous
            # green would retire an open item in arch's spec on the strength of
            # a test that could not have failed. But it must not report FAIL
            # either: "the transport died on idle" and "we could not hold the
            # link quiet long enough to find out" are opposite findings, and a
            # shared red would make the first invisible behind the second. So
            # the preconditions are tracked SEPARATELY and exit 2.
            idle_inconclusive.update({
                "the quiet window could have expired the mapping": ok_pre,
                "the app was actually quiet": not app_chatty,
            })

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
            # The diagnosis assertions are load-bearing in this mode, so they
            # must reach the verdict — a check that only prints is a check that
            # can regress silently.
            diagnosis = [k for k in checks if "WHY it is unreachable" in k
                         or "reflector, not a relay" in k]
            diag_ok = all(checks[k] for k in diagnosis) and bool(diagnosis)
            delivery = [k for k in checks if "delivered" in k]
            rv_ok = all(checks[k] for k in rendezvous)
            delivered = any(checks[k] for k in delivery)
            print("\n── negative control: two isolated networks ───")
            print(f"   rendezvous over the node (must hold) : {'✅' if rv_ok else '❌'}")
            print(f"   media delivered (must NOT)           : {'❌ delivered' if delivered else '✅ no path'}")
            print(f"   the app SAYS why (no-reflector)      : {'✅' if diag_ok else '❌'}")
            ok = rv_ok and not delivered and diag_ok
            if not diag_ok:
                print("\nRESULT: FAIL ❌ unreachable, and the app did not say why.")
                print("  This rig is the `NoReflector` topology; the Chat header")
                print("  must name the reflector as the missing piece. Silence")
                print("  here is the bug the classifier exists to fix.")
            elif delivered:
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
        if ok and IDLE_SECS and not all(idle_inconclusive.values()):
            # Everything asserted passed; what failed is this run's standing to
            # assert anything about idle. Exit 2 so a caller can tell the three
            # states apart — and say plainly which claim is void, because
            # "survives idle" is the one an open spec item is waiting on.
            print("\nRESULT: INCONCLUSIVE ⚠  the transport behaved correctly, but "
                  "this run cannot claim `survives idle`:")
            for k, v in idle_inconclusive.items():
                if not v:
                    print(f"   ⚠  {k}")
            print("   The delivery and no-rebuild results above stand on their own;"
                  "\n   what is NOT established is that they were obtained across a"
                  "\n   silence long enough for the NAT mapping to have died.")
            return 2
        return 0 if ok else 1
    finally:
        rq(A_BASE, "DELETE", f"/session/{sa}"); rq(B_BASE, "DELETE", f"/session/{sb}")

if __name__ == "__main__":
    sys.exit(main())

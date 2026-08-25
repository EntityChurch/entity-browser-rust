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
                hits[n].append(l[:1300])
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

        # `get` is a REAL system/tree operation (core/tree/src/lib.rs:428,457:
        # get|put|snapshot|diff|merge|extract). The prior default `list` does not
        # exist there and the handler correctly returned 400 unknown_operation —
        # so every run failed at the APPLICATION layer regardless of transport.
        op = sys.argv[2] if len(sys.argv) > 2 else "get"
        cmd_a = f"exec entity://{pb}/system/tree {op}"
        cmd_b = f"exec entity://{pa}/system/tree {op}"

        # --- SYMMETRIC concurrent drive: both must exec toward each other or it
        #     deadlocks (glare role by peer-id order). Submit back-to-back so both
        #     establish_live() calls are in flight together. ---
        print(f"\nA> {cmd_a}\nB> {cmd_b}")
        print("submit A:", submit(A_BASE, sa, cmd_a), " submit B:", submit(B_BASE, sb, cmd_b))

        # --- poll both scrollbacks; print progress so we see pending vs error ---
        window_s = float(sys.argv[3]) if len(sys.argv) > 3 else 45.0
        settle = lambda t: any(k in t.lower() for k in ["error", "fail", "timed out", "timeout", "revision", "children", '"path"', "entries", "status=", "no originating authority"])
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

        # --- PEER VANTAGE: the §6.5 lines the establisher now emits (485e269),
        #     forwarded worker→main by the BroadcastChannel. The derived pair
        #     rendezvous key (debug!, on because we boot with log=trace) matches
        #     byte-for-byte against the node's line; the failure warn! names
        #     WHICH failure (VerificationUnavailable=mixed build, IdentitySkew,
        #     Timeout=unshared-bucket-or-absent-peer). Grep AFTER the drive —
        #     establish_live() runs during the exec, not at boot. ---
        # The §6.5 negotiation band (worker-realm, forwarded). Since core-rust's
        # 40294ed the Timeout warn carries the TERMINAL STATE (role / sdp_exchange
        # / channel wait) instead of a guess, and a per-tick `trace!` fires every
        # poll — so the LAST "negotiation tick" a peer logs, with offered/answered/
        # counterpart_msgs, says where it stalled. (Note: "data channel did not
        # open within Nms" is NOT a log line — it is a value returned across the
        # worker boundary and dropped by `if let Ok(channel) = io.wait_open(..)`;
        # our earlier grep for it was futile, per core-rust. Read the tick trace +
        # the Timeout Display instead.)
        needles = ["§6.5", "rendezvous_key", "derived pair", "VerificationUnavailable",
                   "IdentitySkew", "MIXED BUILD", "no live path",
                   "negotiation window closed", "negotiation tick",
                   "sdp_exchange", "channel wait", "role=",
                   # 3dcd484 instruments — the branch pickers:
                   #   fed=0            → core-rust's candidates_for (trickle drop)
                   #   fed>0, ice=New   → candidates never reached the agent
                   #   fed>0, ice=Failed→ pairs tried, none worked (connectivity/timing)
                   #   ice=Connected    → trickle fine; DTLS/SCTP (and the control's
                   #                      non-trickle/one-shot gap was what mattered)
                   "candidates posted", "posted=", "fed=", "ice=", "conn="]
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            hits, n = grep_log(base, sid, needles)
            printed = [(k, v) for k, v in hits.items() if v]
            print(f"\n[{lbl}] §6.5 establisher lines ({n} log lines total):")
            if not printed:
                print("     (none — establish_live never derived a key on this peer;"
                      " check boot/provisioning above)")
            for k, v in printed:
                # For the per-tick trace the LAST ticks show where it stalled;
                # for one-shot warns the first hit is the whole story.
                show = v[-4:] if k == "negotiation tick" else v[:3]
                for line in show:
                    print(f"   {k}:", line)
        # --- HONEST GATE — rung-1 = ONE real cross-peer round-trip over WebRTC ---
        # The prior gate keyed PASS on settle(), which matches "error"/"timeout" as
        # "settled" — so a 400/401/timeout printed PASS ✅ (core-rust flagged this 3×).
        # Replaced with two independent proofs, BOTH required:
        #   (1) the data channel is OPEN on both peers (transport up), and
        #   (2) at least one direction carried a real round-trip: the request crossed
        #       the channel, the handler ran, and a tree payload came back (not an error).
        # The §6.5 acceptor-authority gap (ask 2) means the reverse direction legitimately
        # cannot originate yet — that is surfaced as a KNOWN gap, not hidden, not a failure.
        def channel_open(base, sid):
            hits, _ = grep_log(base, sid, ["data channel is OPEN", "live path established"])
            return any(v for v in hits.values())

        def classify(tail):
            t = tail.lower()
            if "no originating authority" in t:
                return "auth-blocked"          # KNOWN §6.5 gap — upstream ruling pending
            # A real success is an explicit 2xx status crossing back from the REMOTE
            # handler over the channel — observed shape:
            #   `status=200 type="system/handler" ... {"interface": "system/handler/..."}`
            # or a get that returns tree data. `status=2` is the unambiguous signal.
            # Do NOT key on `type="system/` or `+1 included` — BOTH appear in error
            # responses too (`type="system/protocol/error"`, `... +1 included`), which
            # once mis-passed a 403. Body markers below never appear in error bodies.
            if ("status=2" in t
                    or any(s in t for s in ['"interface"', "revision", "entries",
                                            "content_hash", "entity_type", '"path"',
                                            "listing"])):
                return "success"               # a real payload crossed back from the remote
            if any(e in t for e in ["unknown_operation", "unresolvable", "not_found",
                                    "not found", "status=4", "status=5", " 400", " 401",
                                    "invalid_params", "timed out", "timeout",
                                    "no transport profile", "no live path", "error", "failed"]):
                return "error"
            return "pending"

        open_a, open_b = channel_open(A_BASE, sa), channel_open(B_BASE, sb)
        cls_a, cls_b = classify(a_tail), classify(b_tail)
        success_dirs = [d for d, c in (("A", cls_a), ("B", cls_b)) if c == "success"]
        blocked_dirs = [d for d, c in (("A", cls_a), ("B", cls_b)) if c == "auth-blocked"]
        errored_dirs = [d for d, c in (("A", cls_a), ("B", cls_b)) if c == "error"]
        print("\n── rung-1 gate ─────────────────────────────────────")
        print(f"   channel OPEN:  A={open_a}  B={open_b}")
        print(f"   A> {op} -> {cls_a}    B> {op} -> {cls_b}")
        if blocked_dirs:
            print(f"   NOTE: dir(s) {blocked_dirs} = KNOWN §6.5 symmetric-originate gap — the")
            print(f"         acceptor holds no capability granted BY the remote (only one it")
            print(f"         minted FOR it). Upstream ruling pending (core-rust SPEC-AMBIGUITIES")
            print(f"         '§6.5 symmetric originate'). Not counted against rung-1.")
        # PASS: transport up on BOTH peers AND BOTH directions carried a real
        # round-trip. §6.5 mutual minting (PROPOSAL-SYMMETRIC-REENTRY-MUTUAL-
        # MINTING) makes the answerer→offerer direction work too, so bidirectional
        # is now the bar — a lone-direction pass means the reciprocal grant did not
        # land (or verify) and is a FAIL, not a known gap.
        both_open = open_a and open_b
        ok = both_open and len(success_dirs) == 2
        if ok:
            print("\nRESULT: PASS ✅ BIDIRECTIONAL cross-peer WebRTC round-trip "
                  "(both directions status=200 over one §6.5 data channel).")
        else:
            why = []
            if not both_open:
                why.append(f"channel not open (A={open_a} B={open_b})")
            if len(success_dirs) != 2:
                why.append(f"not bidirectional (A={cls_a} B={cls_b}; success={success_dirs})")
            if blocked_dirs:
                why.append(f"dir(s) {blocked_dirs} still 'no originating authority' — "
                           f"reciprocal §6.5 grant did not land")
            print(f"\nRESULT: FAIL ❌ — {'; '.join(why)} (see tails + signaling log)")
        return 0 if ok else 1
    finally:
        rq(A_BASE, "DELETE", f"/session/{sa}")
        rq(B_BASE, "DELETE", f"/session/{sb}")

if __name__ == "__main__":
    sys.exit(main())

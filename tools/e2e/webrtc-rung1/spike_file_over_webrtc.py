#!/usr/bin/env python3
"""FILE OVER WEBRTC — two browsers meet at a name, then one **serves a file** to
the other. The stretch goal's payload, over the substrate the other gates prove.

`spike_meet_then_chat.py` proves *name → id → connection → message*. This spike
reuses that whole ladder verbatim (it imports it — same harness, same
retry-until-the-app-responds discipline, no second copy to drift) and replaces
only the last phase: instead of a chat message, **A offers a file and B pulls
it**.

Why that is a different claim from chat, and worth its own gate:

  - A chat message is a tree entity that fits in one dispatch. A file is
    **hash-addressed content**: the sender ingests a blob + chunks into
    `system/content`, and the receiver has to walk the closure — fetch the blob,
    decode the chunk list it names, fetch the chunks, reassemble. The payload
    here is deliberately **multi-chunk** (`SIZE` spans several 256 KiB chunks),
    so a receiver that only handled a single self-contained response fails.
  - Until now the serving side of a transfer was native-only: every transfer op
    targets `entity://{peer}/local/files`, whose handler is
    `#![cfg(not(target_arch = "wasm32"))]`. Two browsers had nobody to receive.
    This asserts a **browser** served it.

**Nothing but WebRTC can carry it, and that is structural rather than asserted.**
A and B never learn each other's address — they are handed no URL params, they
add only the signaling *node* to their connector registry, and they meet at a
name. There is no WebSocket between them to fall back to, so bytes that arrive
crossed the §6.5 data channel. (The `data channel is OPEN` log line is checked
too, as a diagnostic; the structural argument is what makes the gate mean
something.)

The Shell verbs (`offer` / `offers` / `pull`) are the driving surface because
they are the transport-agnostic model with no window in the way — the same
`file_offer` flows the File Transfer window will call. `offer <name>
size=<bytes>` generates **deterministic** bytes, so both sides compute the same
offer id and this script can assert on an id it did not have to be told.

Preconditions are `rung1_repro.sh`'s, and it is the runner:
`make e2e-webrtc-file`.
"""
import os, re, sys, time

import spike_meet_then_chat as meet

# Bytes to offer. 700 000 > 2 × 256 KiB, so the blob names three chunks and the
# receiver's closure walk (blob → chunk list → chunks) is genuinely exercised.
# A one-chunk payload would pass with the walk deleted, which is the whole
# failure this size exists to prevent.
SIZE = int(os.environ.get("FILE_SIZE", "700000") or 700000)
NAME = os.environ.get("FILE_NAME", "report.bin").strip()

# The second file, offered from the WINDOW's picker instead of the Shell. A
# different size (so a different content id) and a different name, so nothing in
# phases 7–9 can pass on the strength of the Shell's offer still being there.
# Still multi-chunk, for the same reason `SIZE` is.
WINDOW_SIZE = int(os.environ.get("WINDOW_FILE_SIZE", "700001") or 700001)
WINDOW_NAME = os.environ.get("WINDOW_FILE_NAME", "from-window.bin").strip()

A_BASE, B_BASE = meet.A_BASE, meet.B_BASE
ex, run_until, type_once = meet.ex, meet.run_until, meet.type_once
SHELL_TEXT = meet.SHELL_TEXT

# `offered <name> <n> bytes  id <hex>` / `pulled …` — the two lines the verbs
# print. Captured with the id, because the id is the content hash: the same id
# on both sides is the strongest single statement this gate can make.
def _root():
    return ("const l=document.getElementById('dom-layer');const r=l.shadowRoot||l;")


def ROW_NAMED(name):
    """Is there a File Transfer row for this filename? (Its tree key is a
    content hash for an offer, so the NAME is the only stable handle — which is
    the manifest's whole purpose.)"""
    return _root() + f"const e=r.querySelector('[data-row-name=\"{name}\"]');return !!e;"


def FIELD_TEXT(field):
    return _root() + f"const e=r.querySelector('[data-field=\"{field}\"]');return e?e.textContent:'';"


def OWN_OFFER_ROW(name):
    """Is this file listed in *our own* "Files you are offering" table? The
    sender's view — the only place a person can see what strangers may read
    from them."""
    return _root() + (
        f"const e=r.querySelector('[data-field=\"ft-offer-row\"]"
        f"[data-offer-name=\"{name}\"]');return !!e;"
    )


REFRESH_CLICK = _root() + (
    "const e=r.querySelector('[data-field=\"ft-refresh\"]');"
    "if(!e)return 'no-el';e.click();return 'clicked';"
)


def OFFER_VIA_PICKER(name, size):
    """Drive the window's file picker.

    A harness cannot answer a native file dialog, so it assigns the input's
    `files` and dispatches `change` — the exact point a real choice enters the
    app, with the whole `array_buffer()` → `Action::OfferFile` path downstream
    of it unchanged. The bytes match the Shell verb's generator
    (`(i * 31) % 251`, `parse_offer_payload`), so a size here yields the same
    content id it would there — which keeps this phase's offer id checkable
    against the model's own arithmetic if it ever needs to be."""
    return _root() + (
        "const inp=r.querySelector('[data-field=\"ft-offer-input\"]');"
        "if(!inp)return 'no-input';"
        f"const n={size};const a=new Uint8Array(n);"
        "for(let i=0;i<n;i++)a[i]=(i*31)%251;"
        f"const f=new File([a],'{name}',{{type:'application/octet-stream'}});"
        "const dt=new DataTransfer();dt.items.add(f);inp.files=dt.files;"
        "inp.dispatchEvent(new Event('change'));return 'sent';"
    )


def click_field(base, sid, selector):
    """Click the first element matching `selector`, in the page's own DOM.

    Deliberately a scripted click rather than a WebDriver element click: the
    window re-renders on every repaint, so an element handle taken a moment ago
    is routinely stale — and a stale-element exception here would read like the
    button did nothing."""
    script = _root() + f"const e=r.querySelector('{selector}');if(!e)return 'no-el';e.click();return 'clicked';"
    return ex(base, sid, script)


# The Firefox profile must not stop on a save dialog: the Pull button hands the
# bytes to the browser as a download, and a modal would leave the run hanging on
# something that is not the app's behaviour.
meet.CAPS["capabilities"]["alwaysMatch"]["moz:firefoxOptions"]["prefs"].update({
    "browser.download.folderList": 2,
    "browser.download.dir": "/tmp",
    "browser.download.useDownloadDir": True,
    "browser.helperApps.neverAsk.saveToDisk":
        "application/octet-stream,application/binary,text/plain",
})

OFFERED = re.compile(r"offered\s+(\S+)\s+(\d+)\s+bytes\s+id\s+([0-9a-f]+)")
PULLED = re.compile(r"pulled\s+(\S+)\s+(\d+)\s+bytes\s+id\s+([0-9a-f]+)")
LISTED = re.compile(r"offer\s+(\S+)\s+(\d+)\s+bytes\s+id\s+([0-9a-f]+)")


def wait_for(base, sid, pattern, budget=90, label=""):
    """Poll the Shell scrollback until `pattern` matches. Returns the match or
    None. Polling, not `sleep(fixed)` — an ingest + a closure walk take as long
    as they take, and a fixed guess is the load-dependent flake shape the suite
    keeps converting away."""
    t0 = time.time()
    for i in range(budget):
        m = pattern.search(ex(base, sid, SHELL_TEXT) or "")
        if m:
            print(f"  {label} at t={time.time() - t0:.1f}s: {m.group(0)}")
            return m
        # A failure line is an answer too — stop waiting for a success that is
        # not coming, and print what the app actually said.
        text = ex(base, sid, SHELL_TEXT) or ""
        for word in ("offer failed", "pull failed", "offers failed"):
            if word in text:
                tail = text[text.rfind(word):][:300]
                print(f"  {label} FAILED at t={time.time() - t0:.1f}s: {tail}")
                return None
        time.sleep(1)
    print(f"  {label} did not appear within {budget}s")
    return None


def main():
    node_peer = sys.argv[1]
    print(f"MODE={meet.MODE}   file: {NAME} ({SIZE} bytes, "
          f"~{-(-SIZE // (256 * 1024))} chunks)")
    print(f"signaling node: {node_peer}   tag: {meet.TAG!r}")
    print("NOTE: the browsers are handed no addresses — the ONLY path between "
          "them is the §6.5 data channel.\n")

    sa = meet.new_session(A_BASE)
    sb = meet.new_session(B_BASE)
    checks = {}
    try:
        if not (meet.wait_boot(A_BASE, sa, "A") and meet.wait_boot(B_BASE, sb, "B")):
            return 1

        print("\n── 1. add the connector, then reload ──────────")
        if not (meet.provision(A_BASE, sa, node_peer, "A")
                and meet.provision(B_BASE, sb, node_peer, "B")):
            print("\nRESULT: FAIL ❌ could not register the connector")
            return 1
        meet.goto(A_BASE, sa)
        meet.goto(B_BASE, sb)
        if not (meet.wait_boot(A_BASE, sa, "A") and meet.wait_boot(B_BASE, sb, "B")):
            return 1
        checks["an establisher installs from the registry"] = (
            meet.log_has(A_BASE, sa, "establisher") and meet.log_has(B_BASE, sb, "establisher")
        )

        meet.open_shell(A_BASE, sa, "A")
        meet.open_shell(B_BASE, sb, "B")
        pa = meet.bound_peer_id(A_BASE, sa, "A")
        pb = meet.bound_peer_id(B_BASE, sb, "B")
        print(f"  peer A = {pa}\n  peer B = {pb}")
        if not (pa and pb and pa != pb):
            print("\nRESULT: FAIL ❌ distinct peer ids required")
            return 1
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            if not meet.until_listed(base, sid, "connector ls", "●", tries=3):
                meet.provision(base, sid, node_peer, lbl)

        print(f"\n── 2. both `meet tag {meet.TAG}` ──────────────")
        for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
            started, _ = meet.run_until(base, sid, f"meet tag {meet.TAG}",
                                        meet.shell_says(base, sid, "meeting at"), tries=10)
            print(f"  {lbl} meet started: {started}")
        learned_a, learned_b = [], []
        for i in range(45):
            time.sleep(1)
            learned_a, learned_b = meet.met_ids(A_BASE, sa), meet.met_ids(B_BASE, sb)
            if pb in learned_a and pa in learned_b:
                print(f"  both sides learned the other's id at t={i + 1}s")
                break
        checks["A met B's actual peer id"] = pb in learned_a
        checks["B met A's actual peer id"] = pa in learned_b
        if not (pb in learned_a and pa in learned_b):
            print("\nRESULT: FAIL ❌ the meet did not introduce the two peers")
            return 1

        # ── 3. A offers ──────────────────────────────────────────────────
        # Local work only (chunk + ingest + manifest): nothing crosses yet, so a
        # failure here is about the browser being able to SERVE at all, not
        # about connectivity. Worth separating — the two look identical from a
        # "the file didn't arrive" report.
        print(f"\n── 3. A offers {NAME} ({SIZE} bytes) ──────────")
        type_once(A_BASE, sa, "Shell", "shell-input", f"offer {NAME} size={SIZE}")
        m_offer = wait_for(A_BASE, sa, OFFERED, budget=90, label="A offered")
        checks["a BROWSER can serve a file (ingest + manifest)"] = bool(m_offer)
        if not m_offer:
            print(f"\n[A] scrollback:\n{ex(A_BASE, sa, SHELL_TEXT)}")
            print("\nRESULT: FAIL ❌ the offer never landed")
            return 1
        offer_id = m_offer.group(3)
        checks["the offered size is the size we asked for"] = int(m_offer.group(2)) == SIZE

        # ── 4. B sees the offer ──────────────────────────────────────────
        # The manifest is a tree read across the data channel: this is the
        # first byte of the transfer to actually cross, and it separates "the
        # link is dead" from "the content walk is broken" in the phase below.
        #
        # **A is never told to dispatch, and that is the assertion.**
        # `establish_live` runs only when the peer *itself* consults the §10.3
        # ladder; nothing polls the pair's rendezvous bucket on the strength of
        # someone else having deposited an offer there. So a peer that only
        # serves never negotiates, and the puller's offers sit unanswered —
        # measured on this rig before `reach_keeper` existed: B deposited 52
        # offers, A collected 0 on that key, every negotiation dying
        # `sdp_exchange=INCOMPLETE, fed=0`, and caller-side retry did NOT fix
        # it. Chat never meets this because both sides poll each other at 5 Hz.
        #
        # `src/reach_keeper.rs` is the app's answer: meeting someone registers
        # the intent to be reachable to them, and while they are not connected
        # this peer probes them on a slow cadence — so A attempts because it
        # met B, not because a test typed a command at it. **The mutation check
        # for the keeper is this phase**: an earlier revision typed
        # `offers {pb}` into A here, and deleting that line is what turned the
        # gate red before the keeper and leaves it green after.
        print(f"\n── 4. B lists A's offers ──────────────────────")
        type_once(B_BASE, sb, "Shell", "shell-input", f"offers {pa}")
        m_list = wait_for(B_BASE, sb, LISTED, budget=120, label="B listed")
        checks["the offer manifest crossed to B"] = bool(m_list)
        checks["B sees the same content id A published"] = (
            bool(m_list) and m_list.group(3) == offer_id
        )

        # ── 5. B pulls the bytes ─────────────────────────────────────────
        print(f"\n── 5. B pulls {NAME} ──────────────────────────")
        type_once(B_BASE, sb, "Shell", "shell-input", f"pull {pa} {NAME}")
        m_pull = wait_for(B_BASE, sb, PULLED, budget=180, label="B pulled")
        checks["B pulled the file"] = bool(m_pull)
        # Byte-for-byte, twice over: the count matches AND the id does. The id
        # is the blob's content hash and `reassemble` walks it, so bytes that
        # arrive under it cannot be other bytes — a truncated or corrupted
        # transfer fails to reassemble rather than reporting a short success.
        checks["every byte arrived"] = bool(m_pull) and int(m_pull.group(2)) == SIZE
        checks["the content hash matches what A offered"] = (
            bool(m_pull) and m_pull.group(3) == offer_id
        )

        # ── 6. the WINDOW, which is what a person actually uses ──────────
        # Everything above drives Shell verbs — the model with no UI in the
        # way. This drives the File Transfer window instead, and it is a
        # different claim in three places, each of which was broken until it
        # was tested rather than assumed:
        #
        #   (a) the target list reads the "ever connected" registry, which only
        #       the manual Connect button used to write — so a peer met by NAME
        #       appeared nowhere, and the window said there was nobody to
        #       transfer with while the bytes were already crossing;
        #   (b) an offered file has to render as an ordinary row under its
        #       FILENAME (its tree key is a content hash);
        #   (c) Pull has to know it is a content walk rather than a
        #       `local/files:read` — decided in the model, so the button and
        #       the DOM stay peer-kind-blind.
        print("\n── 6. the File Transfer WINDOW ────────────────")
        ex(B_BASE, sb, meet.spawn_script("File Transfer"))
        row = None
        for i in range(60):
            time.sleep(1)
            row = ex(B_BASE, sb, ROW_NAMED(NAME))
            if row:
                print(f"  B's window lists {NAME} at t={i + 1}s")
                break
        checks["the window lists the offered file by name"] = bool(row)

        saved = False
        if row:
            # Select the row, then Pull. Retried: a spawn only queues the
            # window, and the row is re-rendered on every repaint, so an
            # element found a moment ago can be stale.
            for _ in range(20):
                click_field(B_BASE, sb, f'[data-row-name="{NAME}"]')
                time.sleep(0.5)
                click_field(B_BASE, sb, '[data-field="ft-pull"]')
                for _ in range(10):
                    time.sleep(1)
                    out = ex(B_BASE, sb, FIELD_TEXT("ft-results")) or ""
                    if "✓ saved" in out and NAME in out:
                        saved = True
                        break
                if saved:
                    break
            print(f"  B's window saved the file: {saved}")
            if not saved:
                print(f"  results pane: {(ex(B_BASE, sb, FIELD_TEXT('ft-results')) or '')[-400:]!r}")
        checks["the window pulls it (Pull → saved)"] = saved

        # ── 7. A OFFERS from the window, with no Shell at all ────────────
        # Phase 3 offered through `offer <name> size=…`, which is the model
        # with no UI in the way — and a CLI a person does not have. This is the
        # same act through the shipped surface: the picker in A's own File
        # Transfer window.
        #
        # The file input is driven directly rather than by clicking the button,
        # because the button opens a NATIVE file dialog no harness can answer.
        # Assigning `files` and dispatching `change` enters the app at exactly
        # the point a real choice does — everything from the `change` listener
        # onward is the shipped path, including the `array_buffer()` read.
        print(f"\n── 7. A offers {WINDOW_NAME} from the WINDOW ─")
        ex(A_BASE, sa, meet.spawn_script("File Transfer"))
        sent = None
        for _ in range(30):
            time.sleep(1)
            sent = ex(A_BASE, sa, OFFER_VIA_PICKER(WINDOW_NAME, WINDOW_SIZE))
            if sent == "sent":
                break
        print(f"  picker driven: {sent}")
        own_row = False
        for i in range(60):
            time.sleep(1)
            own_row = ex(A_BASE, sa, OWN_OFFER_ROW(WINDOW_NAME))
            if own_row:
                print(f"  A's window lists its own offer at t={i + 1}s")
                break
        # Two distinct claims: the window can MAKE an offer, and it shows the
        # person what they are now serving — which is the only place anyone can
        # see what a stranger may read from them.
        checks["the window can offer a file (no Shell)"] = bool(own_row)

        # ── 8. B pulls the window's offer ────────────────────────────────
        # Refresh rather than reopen: the offer landed after B's window listed
        # A, so this also proves Refresh re-asks for offers.
        print(f"\n── 8. B pulls {WINDOW_NAME} ──────────────────")
        w_row = False
        for i in range(60):
            ex(B_BASE, sb, REFRESH_CLICK)
            time.sleep(1)
            w_row = ex(B_BASE, sb, ROW_NAMED(WINDOW_NAME))
            if w_row:
                print(f"  B's window lists {WINDOW_NAME} at t={i + 1}s")
                break
        checks["B sees the window-made offer"] = bool(w_row)

        w_saved = False
        if w_row:
            for _ in range(20):
                click_field(B_BASE, sb, f'[data-row-name="{WINDOW_NAME}"]')
                time.sleep(0.5)
                click_field(B_BASE, sb, '[data-field="ft-pull"]')
                for _ in range(10):
                    time.sleep(1)
                    out = ex(B_BASE, sb, FIELD_TEXT("ft-results")) or ""
                    if "✓ saved" in out and WINDOW_NAME in out:
                        w_saved = True
                        break
                if w_saved:
                    break
            if not w_saved:
                print(f"  results pane: {(ex(B_BASE, sb, FIELD_TEXT('ft-results')) or '')[-400:]!r}")
        checks["B pulls what the window offered"] = w_saved

        # ── 9. A stops offering, and B stops seeing it ───────────────────
        # A listing that only ever *adds* cannot show a withdrawal: Refresh
        # would fetch the shorter list, insert nothing, and leave a dead row
        # with a Pull button behind it. This is that assertion, driven from the
        # far side (A's button), which is the only shape that can fail if the
        # cache is merge-only.
        print(f"\n── 9. A stops offering {WINDOW_NAME} ─────────")
        click_field(A_BASE, sa, f'[data-field="ft-stop-offer"][data-offer-name="{WINDOW_NAME}"]')
        gone_a = False
        for _ in range(30):
            time.sleep(1)
            if not ex(A_BASE, sa, OWN_OFFER_ROW(WINDOW_NAME)):
                gone_a = True
                break
        checks["A's own list drops what it withdrew"] = gone_a
        gone_b = False
        for _ in range(60):
            ex(B_BASE, sb, REFRESH_CLICK)
            time.sleep(1)
            if not ex(B_BASE, sb, ROW_NAMED(WINDOW_NAME)):
                gone_b = True
                break
        checks["B's listing drops it on the next refresh"] = gone_b

        # ── 10. it really was the data channel ───────────────────────────
        # Structural first: neither browser was ever given the other's address,
        # so no WebSocket between them exists. This log check is the visible
        # confirmation of the same thing.
        opened = (meet.log_has(A_BASE, sa, "data channel is OPEN")
                  and meet.log_has(B_BASE, sb, "data channel is OPEN"))
        print(f"\n  a §6.5 data channel opened on both sides: {opened}")
        checks["it crossed a WebRTC data channel"] = opened

        print("\n── file-over-webrtc gate ─────────────────────")
        for k, v in checks.items():
            print(f"   {'✅' if v else '❌'}  {k}")
        ok = all(checks.values())
        if not ok:
            for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
                print(f"\n[{lbl}] shell scrollback:\n{ex(base, sid, SHELL_TEXT)}")
            # The establishment diagnostic the other gates print: a failed
            # traversal and a failed rendezvous are indistinguishable without it.
            print("\n── §6.5 establishment (failures per side) ────")
            for base, sid, lbl in ((A_BASE, sa, "A"), (B_BASE, sb, "B")):
                fails = [l for l in meet.log_lines(base, sid)
                         if "negotiation to" in l and "failed" in l]
                print(f"  {lbl}: {len(fails)} failed"
                      + (f"\n     first: {fails[0][:300]}" if fails else ""))
        print(f"\nRESULT: {'PASS ✅ a file crossed between two browsers' if ok else 'FAIL ❌'}")
        return 0 if ok else 1
    finally:
        meet.rq(A_BASE, "DELETE", f"/session/{sa}")
        meet.rq(B_BASE, "DELETE", f"/session/{sb}")


if __name__ == "__main__":
    sys.exit(main())

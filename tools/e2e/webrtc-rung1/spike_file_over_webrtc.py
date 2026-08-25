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

A_BASE, B_BASE = meet.A_BASE, meet.B_BASE
ex, run_until, type_once = meet.ex, meet.run_until, meet.type_once
SHELL_TEXT = meet.SHELL_TEXT

# `offered <name> <n> bytes  id <hex>` / `pulled …` — the two lines the verbs
# print. Captured with the id, because the id is the content hash: the same id
# on both sides is the strongest single statement this gate can make.
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
        # **A dispatches too, and that is not decoration — it is a measured
        # requirement.** `establish_live` runs only when the peer *itself*
        # consults the §10.3 ladder; nothing polls the pair's rendezvous bucket
        # on the strength of someone else having deposited an offer. So a peer
        # that only ever receives never negotiates, and the puller's offers sit
        # unanswered: measured here as B depositing 52 offers to A's 0 collects
        # on that key, every negotiation dying `sdp_exchange=INCOMPLETE,
        # fed=0`. Chat never meets this because BOTH sides poll each other at
        # 5 Hz — the serving side's attempt is a side effect of its own
        # delivery loop. Giving A a harmless read toward B (it has no offers;
        # the dispatch is the point) is the same mutuality, made explicit.
        # The app-side answer — who keeps the serving side attempting while an
        # offer stands — is a Track A item this gate exists to keep visible.
        print(f"\n── 4. B lists A's offers ──────────────────────")
        type_once(A_BASE, sa, "Shell", "shell-input", f"offers {pb}")
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

        # ── 6. it really was the data channel ────────────────────────────
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

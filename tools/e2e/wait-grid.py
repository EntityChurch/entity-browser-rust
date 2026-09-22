#!/usr/bin/env python3
"""Selenium grid readiness — the two questions `make` needs answered.

    wait-grid.py <url> [timeout_secs]   # WAIT until a slot is free (exit 1 on timeout)
    wait-grid.py <url> --preflight      # is anything there? report slot state, never block

**`ready` means "a slot is free", not "the hub is up".** The grid serves
`/status` from the moment the hub starts, while its node registers a second
or two later — and it flips back to `ready: false` for as long as the one
slot (`SE_NODE_MAX_SESSIONS=1`) is held. Measured 2026-09-03 against a live
run: `ready = False`, `message = "Selenium Grid not ready."`, one slot with a
session on it.

That makes it the right predicate for **`make e2e-grid`**, which has just
created a fresh container and must not hand the suite a node whose node has
not registered yet.

It makes it the WRONG predicate for a hard preflight, and the reason is
specific: a *failing* e2e test panics before `client.close()`, leaking its
session, and `setup()`'s `reap_stale_sessions()` exists to clear exactly
that. A preflight that refused on `ready: false` would refuse the runs the
reaper is there to rescue. So `--preflight` **reports and proceeds** — the
one thing it must not do is stay silent, because an occupied slot otherwise
surfaces 4 minutes later as the suite's stall watchdog naming whichever
phase happened to be running.
"""

import json
import sys
import time
import urllib.request


def status(url):
    """`(reachable, ready, busy_slots)` — never raises."""
    try:
        with urllib.request.urlopen(url.rstrip("/") + "/status", timeout=3) as r:
            v = json.loads(r.read()).get("value", {})
    except Exception:
        return False, False, 0
    busy = sum(
        1 for n in v.get("nodes", []) for s in n.get("slots", []) if s.get("session")
    )
    return True, bool(v.get("ready")), busy


def main(argv):
    url = argv[1]

    if "--preflight" in argv:
        reachable, ready, busy = status(url)
        if not reachable:
            print()
            print(f"e2e preflight: nothing answering on {url} — start Selenium first:")
            print("  make e2e-grid          # a correctly-configured grid on :4444")
            print("  make e2e-grid GRID_PORT=4455 && make e2e-worker WEBDRIVER=http://localhost:4455")
            print("  (details: tools/e2e/README.md)")
            print()
            return 1
        if not ready and busy:
            print(
                f">>> e2e preflight: {url} is up but {busy} slot(s) are IN USE. "
                "The suite reaps stale sessions on setup, so a LEAKED one is fine — "
                "but if another run is live this one will queue and then hit the "
                "stall watchdog. Check with: podman ps"
            )
        elif not ready:
            print(f">>> e2e preflight: {url} answers but reports not-ready (no node registered yet?)")
        return 0

    deadline = int(argv[2]) if len(argv) > 2 else 60
    for i in range(1, deadline + 1):
        if status(url)[1]:
            print(f" — READY after {i}s")
            return 0
        sys.stdout.write(".")
        sys.stdout.flush()
        time.sleep(1)
    print(f" — NEVER became ready within {deadline}s")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))

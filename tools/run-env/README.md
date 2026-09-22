# `tools/run-env` — the run-environment rig

A small Linux machine in a browser tab: Alpine on a 9p root, under v86, with a real terminal.

**Status: research rig, not product.** Nothing in `src/` uses any of this, and nothing here is in
`make`. It is committed so the work is portable and backed up, not because it ships.

**Location is provisional.** It lived in `~/vendor-research/` (outside any repo) through the whole
arc; it is here so a second machine can run it. Whether it ends up in its own repo is open, and
nothing depends on this path — the scripts resolve everything relative to themselves.

---

## Run it

```bash
cd tools/run-env
./fetch-engine.sh                 # v86 + xterm, verified against pinned hashes (~10 MB)
./build-guest.sh                  # Alpine root + kernel + 9p initramfs, ~3 min, in podman
node alpine-guest/boot.js         # headless boot gate: 5 checks, ~10 s, exit 0/1

# and in a browser
python3 probes/plain-serve.py 8201 alpine-guest
python3 probes/alpine-browser-probe.py http://127.0.0.1:8201/   # 7 assertions
```

Host needs `podman`, `python3`, `curl`. Everything that compiles runs in a container, and `node`
does not have to be on the host either — the gate is location-independent, so:

```bash
podman run --rm --security-opt label=disable -v "$PWD:/rig:ro" -w /rig \
  docker.io/library/node:20-slim node alpine-guest/boot.js
```

The browser probes default to a grid on `:4444`; on a shared box use your own
(`make e2e-grid GRID_PORT=4470` from the repo root) and pass `GRID=http://127.0.0.1:4470`.

## What is here

| | |
|---|---|
| `fetch-engine.sh` | fetches v86 + xterm and **verifies every artifact against its pinned sha256** |
| `build-guest.sh` | Alpine root + kernel + 9p initramfs + index, one command, hermetic |
| `build-kernel.sh` | a 1.6 MB kernel with **9p built in**, so there is no initramfs at all |
| `fs2json.py` | a tree → v86's index + a SHA-256-addressed blob store |
| `alpine-guest/index.html` | ⭐ **THE page.** Terminal, key bar, mobile keyboard, `stty`, the entity-app contract, file verbs |
| `alpine-guest/boot.js` | headless boot gate — node, no browser, no grid |
| `alpine-guest/host.html` | iframe host harness (the app-in-a-frame case) |
| `probes/` | browser gate, round-trip, refetch, and three serving modes (plain / no-store / shaped) |
| `archive-pages/` | six retired one-off pages **and a record of what each proved** |

Everything else — `v86-m1/`, `alpine-guest/guest/`, `kernel-min/` — is generated and git-ignored.
177 KB of source; ~300 MB of output.

## Two rules this rig has already paid for

**1. Extend the one page.** Six one-off pages were built in two days, each rebuilt rather than
extended; the worst reinvented a *worse* terminal, dropped the keyboard entirely, and put a JSON
dump across 80% of the viewport — it was a measurement probe that got opened as the product.
`alpine-guest/index.html` is the descendant that keeps every hard-won fix. `archive-pages/README.md`
records what each retired page proved, so the work is kept rather than deleted.

**2. Diagnostics belong to the probe, never to the viewport.** Numbers go to `window.__m1`; a probe
reads them and a person reads the terminal. The status bar is one line.

## Where the writing is

The measurements are the point; this directory is what produced them.
`docs/plans/OVERVIEW-2026-09-10-THE-RUN-ENVIRONMENT-ARC.md` indexes all of it. The two that bear
directly on the scripts here:

- **`MEASUREMENT-2026-09-11-r`** — provenance for all seven layers, every row hash-matched against a
  named upstream. The pins in `fetch-engine.sh` are from it. Also: v86 has **no version and no
  pinnable source anywhere**, so its sha256 *is* the version.
- **`MEASUREMENT-2026-09-11-s`** — 85% of a first visit's fixed cost is `vmlinuz` + `initramfs`, and
  the initramfs exists only because Alpine ships `CONFIG_9P_FS=m`. `build-kernel.sh` builds it in and
  the artifact is deleted rather than trimmed: **12.83 MB → 1.63 MB**.

## Known-open

- **`build-kernel.sh`'s guest does not reach a shell via `/sbin/init`.** The kernel, the 9p root and
  the serial console in both directions all work, and `busybox sh` as PID 1 is fully interactive —
  but Alpine's inittab → `bash -l` logs out immediately when no initramfs has run openrc's sysinit.
  Userland, not kernel; it moves none of the size numbers. See `MEASUREMENT-…-s` §4.
- **Nothing has run on a phone.** It *has* run in a real `sandbox="allow-scripts"` iframe —
  `probes/alpine-roundtrip-probe.py` against `alpine-guest/host.html`, 7/7 (`MEASUREMENT-…-n`) —
  but every browser number is still Firefox on one desktop.
- **Nothing crosses into the entity tree.** `/mnt` is empty; the file verbs are measured against the
  page's own contract, not against a peer.

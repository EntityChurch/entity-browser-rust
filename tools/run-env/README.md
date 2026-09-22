# `tools/run-env` — the run-environment rig

A small Linux machine in a browser tab: Alpine on a 9p root, under v86, with a real terminal.

**Status: research rig, now packaged as an entity-app.** Nothing in `src/` is VM-specific:
the product side is the generic **asset-bundle** host (`src/apps/assets.rs`), and this directory
is one app that uses it. Nothing here is in `make`.

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

## Run it as an app in the Apps window

```bash
./build-packages.sh                                # optional: the package set (after build-guest.sh)
# optional: resume instead of boot (after packages; needs a grid and the page served)
#   (cd ../.. && make serve PORT=8216 DIST=tools/run-env/alpine-guest) &
#   GRID=http://127.0.0.1:4470 python3 build-snapshot.py http://127.0.0.1:8216/
./package-app.sh                                   # → app-dist/: index.json, alpine.html, alpine.assets/{engine,image,packages}/
cd ../.. && cp -r dist dist-vm                     # an SPA build to publish onto (make wasm first)
make site OUT=dist-vm DEPLOY_CONFIG=1 APPS_DIST=tools/run-env/app-dist
make serve PORT=8211 DIST=dist-vm                  # open Apps → Alpine Linux
GRID=http://127.0.0.1:4471 VISITS=2 python3 tools/run-env/probes/apps-window-vm-probe.py
```

**`--ingest-apps` is the whole app set, not an addition.** Publishing `app-dist/` alone to a domain
that serves entity-apps' catalog removes those apps from it. To publish Alpine *beside* them, build one
directory holding both — the base is only read:

```bash
./package-app.sh app-dist-all --onto <entity-apps>/dist   # base entries + alpine in one index.json
make site-dist APPS_DIST=tools/run-env/app-dist-all        # from the repo root
```

**`apk add` works with no network.** `build-packages.sh` fetches `packages.txt` plus its dependency
closure, drops what the image already ships, signs the index with our key (`package-key.sh`), and
indexes it for 9p. It ships as a third bundle, `packages`; the page grafts it into the root at
`/var/cache/entity-packages`, where the image's `/etc/apk/repositories` points, and a visitor
downloads only the files their install reads (`apk add jq`: 3 of 87). The image trusts exactly our
key for it, so an unsigned or re-signed index is refused. **The private key is in `.keys/`,
gitignored — back it up**: without it a new package set cannot be signed for images already out.
No package set built, or a domain that omits the bundle: the machine boots the same and `apk add`
says there is no such package. Installed packages last until reload.

**A launch resumes instead of booting** when the image bundle carries a snapshot
(`build-snapshot.py`): ~10 MiB, shell ready in ~1 s instead of ~10. `snapshot.json` records the
engine, image index, package index and machine settings the snapshot was taken with; the page
resumes only when all four match and boots cold otherwise. A resume skips everything a boot does,
so the agent's `resume` verb puts back what that loses: the clock, the kernel RNG (credited and
force-reseeded with bytes from the page by `entity-reseed` — **without it every visitor reads the
same random bytes**, measured), and the saved `/root` files. **Rebuild the snapshot after
`build-guest.sh` or `build-packages.sh`**; a stale one is never used, only downloaded.

**Your home directory (`~`, `/root`) is kept; everything else starts fresh on restart.** It persists
through the host (`"x-workspace": true`): restored before the machine starts, saved ~1.5 s after each
command (the shell's `PROMPT_COMMAND` pings the page), every 30 s, when the page is hidden, before a
restart, and on `save`. `receive` lands there too, never over an existing file (`notes-1.txt`).
**Power** is the page's ⏻ button — Restart, Restart and show the boot, Turn off, Start — and `reboot` /
`poweroff` / `shutdown -r` in the guest ask the page for the same. `?boot=cold` boots instead of
resuming. Plan §9 has the design and what it cost (notably: `HOME` was `/` until then). The page and the guest talk on a
second serial port (ttyS1) through a small agent (`build-guest.sh` 3d) for window sizes, save
listings and the answers `send`/`save` print — nothing is ever typed into the person's shell.
`VISITS=2` on the probe below proves it end to end.

The page is delivered as an opaque-origin `srcdoc` frame, so it cannot fetch its own files by
URL. The catalog declares two bundles, `"x-assets": ["engine", "image"]`, and the page asks the
player for each key (`x-asset-get`). The player takes them from its own tree or from the
publisher's origin by content hash. The page still runs standalone: with no host offering
bundles in `init`, it fetches by URL exactly as before.

## What is here

| | |
|---|---|
| `fetch-engine.sh` | fetches v86 + xterm and **verifies every artifact against its pinned sha256** |
| `build-guest.sh` | Alpine root + kernel + 9p initramfs + index, one command, hermetic |
| `build-packages.sh` · `packages.txt` · `package-key.sh` | the `apk add` package set: a signed local repository as its own 9p index |
| `build-snapshot.py` | a snapshot taken at the prompt, so a launch resumes (built in the browser, recompressed with zstd) |
| `build-kernel.sh` | a 1.6 MB kernel with **9p built in**, so there is no initramfs at all |
| `fs2json.py` | a tree → v86's index + a SHA-256-addressed blob store |
| `alpine-guest/index.html` | ⭐ **THE page.** Terminal, key bar, mobile keyboard, `stty`, the entity-app contract, file verbs |
| `alpine-guest/boot.js` | headless boot gate — node, no browser, no grid |
| `alpine-guest/host.html` | iframe host harness (the app-in-a-frame case) |
| `package-app.sh` | the rig → an entity-app `dist/` (scripts inlined, two or three asset bundles, hardlinked) |
| `probes/` | browser gate, round-trip, refetch, snapshot measurement, and three serving modes (plain / no-store / shaped) |
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
- **Only `~` persists.** Installed packages and the rest of the root are still RAM over the read-only
  image (a whole-root overlay is after the release). Design §14 and plan §9 have the bounds.

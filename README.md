# Entity Browser (`entity-browser-rust`)

DOM-primary Rust application for the entity system. The cargo package is
`entity-browser-rust`; the active rendering path is HTML DOM.

**Where this sits in the stack.** This is a **binding / app** — a reference
application built on top of the Rust reference implementation
(`entity-core-rust`) and its SDK. It is **one worked example of the paradigm,
not a mandate**: it shows how to build an entity-backed application (a window
manager + content sites + embedded apps, with the entity tree as the single
source of truth), not the only way to do so.

**Active deployment modes:**

- **Web browser** — WASM peer + DOM rendering (`make wasm` / `make serve`).
  Primary focus. The default is a durable main-thread IndexedDB system peer;
  a Worker + OPFS arm is opt-in via `?worker=1`.
- **Tauri desktop** — DOM in a native WebView, with a separate Tauri-side Rust
  backend that can spawn native peers (`make tauri-run`).

There is **no native UI build** — the legacy native renderer was
removed; `make native` prints a redirect to the active targets.

---

## 👉 I want to publish my own site

**Start here: [`docs/PUBLISHING-QUICKSTART.md`](docs/PUBLISHING-QUICKSTART.md).**

Entity Browser is also a **publisher**: point it at a directory of markdown and
it emits one static tree — the app at the apex, your content beside it, all
content-addressed — that you upload to any static host. No server, no database,
nothing dynamic. The quickstart walks the whole path:

```bash
openssl rand -hex 32 > publisher.seed          # your publisher identity — keep it
make site-dist INGEST=../my-content \
     IDENTITY_SEED=$(cat publisher.seed) CONFIG_SITE=my-site
# → dist-site/  — verified, and ready to sync to S3 / R2 / nginx
```

It covers minting the identity, the content format, the flags that matter, the
**two serving properties a CDN must get right** (CORS, and cache headers keyed on
mutability — the one that silently bites), a worked Cloudflare/R2 recipe, safe
republishing, optional name registries, and a symptom→cause table for when it
goes wrong.

> Publishing needs **this source checkout** plus `make` + `podman`. The desktop
> installers ship the *app*, not the publishing tool.

---

---

## How it stores your data — read this before you file a bug

Entity Browser keeps everything in a local entity tree: your windows, your
peers, your sites, and every foreign site you have visited. **Where that tree
lives, and whether it survives, depends on which mode you are in** — and the
difference is visible in the product, so it is worth thirty seconds.

### Browser: one tab owns your data

Your identity is a seed in `localStorage`, shared by every tab on that origin.
Your **tree** is an IndexedDB database — and exactly **one tab may own it at a
time**, elected with a Web Lock. That is deliberate: two tabs writing one store
is last-writer-wins corruption.

| | Tree | Writes | On reload |
|---|---|---|---|
| **First / only tab** — the owner | durable IndexedDB | saved | everything is still there |
| **Every other tab** | **in memory only** | **not saved** | gone |

A secondary tab **says so**, in an amber banner across the top: *"This app is
already open in another tab, which owns your saved data."* Peer creation is
refused there for the same reason.

**What this means in practice, and it is the part that surprises people:** a
second tab starts from nothing. Sites you browsed, registries you pinned, and
publishers you learned about are all in the *owner's* tree, not this one. So a
link that works in your main tab can come up empty in a fresh one — the content
is fine, this tab has just never heard of it. Close the other tab and reload,
and the new tab becomes the owner with all of it back.

`?worker=1` opts into a Worker + OPFS arm instead. It needs a **secure origin**
(`https`, or `localhost`) — on a plain-http address the app silently falls back
to the main-thread arm, which is also why `http://192.168.x.x` deployments have
no QR scanner (`getUserMedia` is gated the same way).

### Desktop (Tauri): two peers, and they are not the same peer

The desktop app is the same web app in a native WebView, plus a **native Rust
backend**. That means two distinct peers, and knowing which is which explains
most desktop questions:

| | Where it lives | Storage | Reachable by others |
|---|---|---|---|
| **WebView peer** — runs the UI, owns your windows/sites | the WebView's IndexedDB | durable (verified on WebKitGTK) | **no** — it cannot bind a socket |
| **Backend peer(s)** — spawned by the Rust side | SQLite under `~/.entity/peers/{name}/` | durable | **yes** — binds `0.0.0.0:4041` |

Two consequences worth knowing up front:

- **`~/.entity/peers/` is shared with every entity-core tool on the machine**,
  not just this app. The desktop only adopts the peers it created (they carry
  `managed_by = "tauri"`); the rest are listed on demand and never touched.
- **On Linux the desktop WebView has no WebRTC** — WebKitGTK ships without
  `RTCPeerConnection`. Finding devices still works, and so does anything over an
  ordinary connection (connect by address, the shared folder, and everything
  served over that connection). What cannot work is reaching the desktop's
  *WebView* peer from a browser with no address to dial. The app shows a red
  banner saying so. Windows and macOS are a different engine and are unmeasured.

Nothing here needs configuring. It is written down because the storage mode
changes what you see, and "it worked in my other tab" is otherwise a mystery.

---

See `CLAUDE.md` for architecture orientation and `docs/architecture/` for the
specs (and `CANONICAL-DOCS.toml` for the curated public reading order).

---

## Repository layout (sibling dependencies)

This crate uses **path dependencies** to other repos in the
`entity-systems/` workspace. They must be cloned as siblings of this
directory:

```
entity-systems/
├── entity-browser-rust/      ← this repo
└── entity-core-rust/         ← required sibling
    └── core/
        ├── ecf/
        ├── hash/
        ├── entity/
        ├── store/
        ├── types/
        ├── peer/
        ├── crypto/
        ├── handler/
        └── capability/
```

If `../entity-core-rust/` is missing or at an incompatible revision,
the build will fail at dependency resolution. This layout is expected
to evolve — eventually these will be published crates — but for now
you need both checkouts side by side.

**Nothing here needs a tag.** Every dependency on `entity-core-rust` is a
`path =` dependency resolved from the sibling directory on disk — there is not a
single `git`/`tag`/`rev` dependency in either manifest, and neither lockfile
carries a `git+` source. Clone the two repos next to each other and `make test`,
`make lint` and `make wasm` work offline against whatever revision you have
checked out.

The one place a tag appears is `CORE_RUST_REF` in
`.github/workflows/release.yml`, and it exists for exactly one reason: a GitHub
Actions runner has **no sibling checkout**, so the release job has to fetch one,
and a tag is the only identifier that is both knowable in advance and publicly
resolvable. It affects release builds only. It is not consulted by any `make`
target, any local build, or any test.

---

## Prerequisites

All toolchain versions are pinned — nothing fetches "latest." The chain:
**mise** (on PATH) → **rustup-init 1.28.2** (pinned + sha256 in `mise.toml`)
→ **Rust 1.94.1** (pinned in `rust-toolchain.toml`, installed by rustup)
→ **trunk 0.21.14** (pinned in `mise.toml`, compiled by cargo).

### 1. System packages

A C compiler is required before anything Rust-related — cargo builds
trunk from source, and trunk pulls C-linker-backed crates. Install this
first.

Fedora:
```bash
sudo dnf install gcc
# Tauri builds additionally need:
sudo dnf install webkit2gtk4.1-devel gtk3-devel \
    libappindicator-gtk3-devel librsvg2-devel \
    openssl-devel pkgconf-pkg-config
```

Debian/Ubuntu:
```bash
sudo apt install build-essential
# Tauri builds additionally need:
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev \
    libssl-dev pkg-config
```

**macOS:** Xcode command-line tools (`xcode-select --install`).

**Windows:** WebView2 runtime (preinstalled on Win11 / current Win10).

### 2. Bootstrap the Rust toolchain (one-time, via mise)

Install [mise](https://mise.jdx.dev/) first (system package manager or
their install script). Then, from this directory:

```bash
# 1. Fetches rustup-init 1.28.2 with checksum verification.
mise install

# 2. Run rustup-init once to set up ~/.cargo and ~/.rustup.
#    --default-toolchain none skips installing a Rust version here;
#    rustup will pick up 1.94.1 from rust-toolchain.toml on first cargo use.
~/.local/share/mise/installs/http-rustup-init/1.28.2/rustup-init \
    --default-toolchain none -y

# 3. Put cargo/rustup on PATH for the current shell (add to your rc file
#    for future shells — rustup-init offers to do this for you).
source ~/.cargo/env

# 4. Re-run mise install. Now that cargo exists, it compiles trunk 0.21.14
#    (~3–5 min; ~400 crates). The first cargo invocation also triggers
#    rustup to download Rust 1.94.1 per rust-toolchain.toml.
mise install
```

After this, `cargo`, `rustc`, `clippy`, `rustfmt`, and `trunk` are all
available at pinned versions. Verify:

```bash
cargo --version   # cargo 1.94.1
rustc --version   # rustc 1.94.1
trunk --version   # trunk 0.21.14
```

### 3. Known unpinned fetch (caveat)

`trunk` itself downloads `wasm-bindgen-cli` on first WASM build to match
the `wasm-bindgen` crate version in `Cargo.lock`. Trunk chooses the
version; it is not pinned in our config. Since `Cargo.lock` is checked
in, the `wasm-bindgen` crate version is fixed, but the CLI binary trunk
fetches to match it comes from trunk's internal resolver. This is the
one remaining link in the chain not directly pinned by us.

---

## Build & run

Two paths, both driven by `make`.

### A. Bare box — `make` + `podman` only (no host toolchain)

The build runs inside a pinned toolchain container (`Dockerfile`: Rust
1.94.1 + the `wasm32` target + Trunk 0.21.14 + binaryen + the webkit2gtk
stack). The parent `entity-systems/` directory is bind-mounted so the sibling
`../entity-core-rust` path-deps resolve; a persistent cargo cache volume keeps
rebuilds fast. **No host `cargo`/`trunk` needed.**

```bash
make build         # alias for `make wasm` — the conventional bare-box entry
make wasm          # WASM debug build → dist/    (in container)
make wasm-release  # WASM release build → dist/  (in container)
make test          # the native suite — unit + peer-integration (in container).
                   # For the CURRENT pass/fail count and binary count, read
                   # docs/STATUS.md — it is re-measured, not quoted
                   # forward, because a number written here goes stale in hours.
make lint          # clippy (in container)
make image         # (re)build the toolchain image explicitly
```

Every `podman build`/`run` is bounded by resource caps so a build cannot take
the host down (see [Resource caps](#resource-caps)).

### B. Host toolchain (native dev — Trunk/cargo on PATH)

The same `make wasm` / `make test` targets work directly if you provision the
host toolchain (see [Prerequisites](#prerequisites)). The serve / demo targets
always run on the host (they need host `python3`):

```bash
make serve         # serve dist/ on :8081 (plain browser, no Tauri)
make build-serve   # build release WASM then serve the latest
make tauri-run     # build WASM + Tauri shell, launch with stdout logs
make site-serve    # publish all demo sites + serve one origin
make native        # prints deprecation redirect (active modes are wasm / tauri)
```

### Building a release

`make dist` builds the shippable installers **for the machine you run it on** —
`.deb` + `.rpm` + `.AppImage` on Linux, `.dmg` on macOS, `.msi` + NSIS `.exe` on
Windows — into `artifacts/`. `make dist-web` produces the browser SPA as a
tarball you can unpack onto any static origin.

```bash
make dist          # containerized (Linux); host needs only make + podman
make dist-native   # host toolchain instead — the only path on macOS/Windows
```

Our tagged releases run exactly these recipes across five platforms
(`.github/workflows/release.yml`), so nothing about building this project
depends on our CI. That workflow file is the authority on how a release is cut
and what each artifact is built from; every published release carries checksums
alongside the installers, so a download can be verified against it.

The browser E2E suite (`make e2e-worker`) is gated behind the `e2e` cargo
feature and needs an external Selenium-firefox container on `:4444` (see
`tools/e2e/README.md`); it is **not** part of the bare-box build gate.

**Always run `make wasm` after changes** — the native test suite
cannot catch WASM-only compilation errors (cfg-gated code, missing
imports, type inference quirks on `wasm32`).

### Resource caps

`make` wires standard per-container ceilings (`PODMAN_BUILD_CAPS` /
`PODMAN_RUN_CAPS`: memory + zero-swap + pids/cpus) into every podman
invocation, so a runaway build OOM-dies cleanly at the cap instead of thrashing
the host into a freeze. The committed defaults are sized to this repo's
measured peak; override per-machine via env vars or an untracked
`caps.local.mk` (e.g. `CAP_MEM=4g CAP_CPUS=2 make build`).

---

## Release posture — research preview

`0.9` is a **research preview**, not a 1.0. It is a worked reference
application on top of the entity substrate, suitable for evaluation and
exploration — not a hardened production deployment.

**Per-version release notes live in [`CHANGELOG.md`](CHANGELOG.md)** — that is
what the GitHub release page is generated from, and it carries the current
feature list and known limitations. This section is only the standing posture.

**Standing caveats — disclosed up front:**

- This is a **development-focused** security posture: backend peers run with
  `debug_open_grants` enabled and transports are plaintext `ws://`. Broader
  production hardening (auth, transport security) is deferred beyond the
  research preview.
- **Browsing a foreign site trusts the serving origin** for the path→hash
  mapping. Content is hash-verified against the pointer that origin served —
  a real gate against corruption and truncation — but the origin supplied the
  pointer, so it is not a defence against a lying host. The signed-root path
  that closes this exists and is reachable through the Shell's `name` verb; it
  is not yet wired into the browsing surface. Stated precisely in
  [`GUIDE-PUBLISHING-AND-NAMES.md` §7](docs/architecture/guides/GUIDE-PUBLISHING-AND-NAMES.md).
- **Storage durability, measured rather than assumed:** the default arm is a
  durable main-thread IndexedDB system peer, verified on Firefox **and** on the
  Tauri WebKitGTK WebView (create a site, relaunch, it is still there). Backend
  peers spawned by the Tauri-side Rust process are durably tree-persisted
  (SQLite under `~/.entity/peers/{name}/`). **Safari / iOS on real hardware is
  still unchecked** — that one is genuinely open.
  *(An earlier version of this section listed WebKitGTK durability as
  unverified. It was, then it was verified by hand, and this note went stale in
  the pessimistic direction.)*

## Notes

- The first backend peer started in the Tauri shell binds to
  `0.0.0.0:4041` and is LAN-accessible by design (development /
  multi-device dogfooding). Subsequent peers bind to dynamic localhost
  ports.
- `Cargo.lock` **is committed**, so dependency versions are pinned for
  reproducible bare-box builds. (An earlier version of this note said it was
  gitignored; that has not been true since it was checked in.)

---

## Supporting the project

This project is developed in the open. If it's useful to you, the best support is
to use it, report issues, and contribute back — see
[CONTRIBUTING.md](CONTRIBUTING.md).

To support the work directly, see the project's funding page.

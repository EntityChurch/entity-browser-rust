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
make test          # 390+ native unit + 17 peer-integration tests (in container)
make lint          # clippy (in container)
make image         # (re)build the toolchain image explicitly
```

Every `podman build`/`run` is bounded by resource caps so a build cannot take
the host down (see [Resource caps](#resource-caps)).

### B. Host toolchain (native dev — Trunk/cargo on PATH)

The same `make wasm` / `make test` targets work directly if you provision the
host toolchain (see [Prerequisites](#prerequisites)). The serve / demo targets
always run on the host (they need host `python3`, and `publish-papers` also
needs `go`):

```bash
make serve         # serve dist/ on :8081 (plain browser, no Tauri)
make build-serve   # build release WASM then serve the latest
make tauri-run     # build WASM + Tauri shell, launch with stdout logs
make publish-serve # publish all demo sites + serve one origin
make native        # prints deprecation redirect (active modes are wasm / tauri)
```

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

## Release notes — `v0.8.0` (research preview)

`0.8` is a **research preview**, not a 1.0. It is a worked reference
application on top of the entity substrate, suitable for evaluation and
exploration — not a hardened production deployment.

**Known caveats — disclosed up front:**

- **Tauri / WebKitGTK IndexedDB durability is _unverified_.** The default
  storage arm is a durable main-thread IndexedDB system peer. That durability
  is **confirmed on Firefox only**; under the Tauri WebKitGTK WebView it is
  **not yet verified** (the durability banner is suppressed there pending a
  `make tauri-run` drive). Treat desktop-WebView persistence as unconfirmed.
  Backend peers spawned by the Tauri-side Rust process *are* durably
  tree-persisted (SQLite under `~/.entity/peers/{name}/`).
- This is a **development-focused** posture: backend peers run with
  `debug_open_grants` enabled and transports are plaintext `ws://`. Broader
  production hardening (auth, transport security) is deferred beyond the
  research preview.

## Notes

- The first backend peer started in the Tauri shell binds to
  `0.0.0.0:4041` and is LAN-accessible by design (development /
  multi-device dogfooding). Subsequent peers bind to dynamic localhost
  ports.
- `Cargo.lock` is currently **gitignored** (see `.gitignore`). For fully
  reproducible bare-box builds, committing it is recommended.

---

## Supporting the project

This project is developed in the open. If it's useful to you, the best support is
to use it, report issues, and contribute back — see
[CONTRIBUTING.md](CONTRIBUTING.md).

To support the work directly, see the project's funding page.

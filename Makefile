# Entity Browser — build targets
#
# Active deployments:
#   make wasm         — browser build (DOM), debug
#   make tauri-run    — desktop build (DOM in WebView + native backend peer)
#
# Release paths (see docs/RELEASE-READINESS.md):
#   make wasm-release — size-optimized browser SPA → dist/ (deploy to a CDN)
#   make publish …    — emit sites/apps content alongside the SPA (CDN deploy)
#   make tauri-bundle  — content-baked desktop app (sites+apps embedded, offline)
#   NOTE: the tauri* targets produce a DEBUG test binary, NOT installers;
#         packaging (.deb/.AppImage/.dmg/.msi via `tauri build`) is unwired — see
#         RELEASE-READINESS §3.
#
# === make + podman build convention ===========================================
# A bare machine needs ONLY `make` and `podman` (no rust/cargo/trunk on host).
# The real deliverable is the browser build: `make wasm` (debug) /
# `make wasm-release`. Both run Trunk INSIDE the toolchain container defined by
# the Dockerfile. The PARENT meta dir is bind-mounted at /src/entity-systems so
# the sibling `../entity-core-rust` workspace path-deps resolve, with workdir set
# to this repo. A persistent cargo registry cache volume makes rebuilds fast.
#
#   make image        — build the toolchain image (rust 1.94.1 + wasm32 + trunk)
#   make build        — alias for `make wasm` (the conventional bare-box entry)
#   make wasm         — debug browser build -> dist/   (in container)
#   make wasm-release — size-optimized release build -> dist/  (in container)
#   make test / lint  — native unit + peer-integration tests / clippy (in container)
#   make publish*     — pure-cargo publish targets (in container)
#
# Host-only targets (NOT part of the bare-box gate, and depend on host services
# or an attached display): the `python3 -m http.server` serve steps (serve /
# build-serve / publish-serve), `e2e-worker` (external Selenium on :4444), and
# `tauri-run` (needs a desktop session). `make native` is a deprecation stub.
IMAGE       := entity-browser-rust-build
PARENT      := $(shell dirname $(CURDIR))
CARGO_CACHE := $(HOME)/.cache/cargo-entity-browser
# Trunk downloads its own version-matched wasm-bindgen-cli into its tool cache
# (`/root/.cache/trunk` in the image). Without persisting it, every `podman run
# --rm` starts cold and re-downloads + re-installs wasm-bindgen — the dominant
# "why is it doing this AGAIN" cost. Persist it like the cargo registry so the
# download happens once. (wasm-opt is baked into the image, not downloaded.)
TRUNK_CACHE := $(HOME)/.cache/cargo-entity-browser-trunk

# Durable publisher identity dir (gitignored). The publish flow loads-or-generates
# `{ENTITY_DATA_DIR}/publish/keypair` so every publish lands under ONE stable
# peer-id (the peer-id is the site address — it must not drift per run). The
# containerized `publish` target runs `podman run --rm`, so `~/.entity` inside the
# container is ephemeral and the key would regenerate every run — we point
# ENTITY_DATA_DIR at this repo-local dir (which rides the existing parent mount) so
# the identity persists. `publish-serve` (host) uses the same dir, so both modes
# publish under the same identity. Override per-machine with PUBLISH_DATA_DIR=…
PUBLISH_DATA_DIR ?= .entity-publish

# Output isolation — so two builds can run AT THE SAME TIME against the same
# source without clobbering each other. `DIST` = trunk's WASM output dir;
# `TARGET_DIR` = cargo's build dir. Both default to the canonical locations,
# so every existing target behaves exactly as before. The `publish-*` family
# overrides them (target-specific vars below) to `dist-publish`/`target-publish`,
# which is what lets `make tauri-run` and `make publish-serve` run concurrently.
# The cargo registry + trunk tool caches stay SHARED (cargo/trunk lock them
# safely, and they're read-mostly once warm) — only the OUTPUT splits, so the
# isolated build is still fast (deps aren't recompiled/redownloaded, only the
# crate's own artifacts live in a separate target dir).
DIST       ?= dist
TARGET_DIR ?= target

# ============================================================================
# Podman resource caps — entity-systems standard (docs/RELEASE-READINESS.md §4).
# Per-container ceilings so a build/run can't take the host
# down. Tune the COMMITTED defaults for THIS project; override per-machine
# WITHOUT editing this file via env vars or an untracked caps.local.mk.
#
#   Precedence (highest first):  env var  >  caps.local.mk  >  defaults below
#   CAP_SWAP == CAP_MEM  =>  zero swap: container is OOM-killed cleanly at the
#   cap instead of thrashing the host into a freeze.
# ============================================================================
-include caps.local.mk          # untracked per-machine overrides (gitignored)

# COMMITTED default sized from this repo's measured peak + headroom. The
# heaviest target is `make test` (595 native unit + 17 peer-integration + the
# other integration suites, compiling + linking the full sibling workspace):
# measured cold worst-case peak ~3.5 GiB at full --cpus=12 (see
# docs/RELEASE-READINESS.md §4). wasm-release peaks lower (~1.9 GiB). 6g leaves room for
# more-core machines + ongoing test growth while staying a hard protective
# ceiling. A smaller machine lowers this via caps.local.mk (§4a).
CAP_MEM           ?= 6g         # hard memory ceiling per container
CAP_SWAP          ?= $(CAP_MEM) # keep == CAP_MEM (no swap); raise only deliberately
CAP_PIDS          ?= 4096       # max procs/threads (RUN only) — stops fork bombs
CAP_CPUS          ?= 6          # CPU cores at runtime (RUN only; fractional ok)
CAP_CGROUP_PARENT ?=            # optional host slice to nest under, e.g. dev-heavy.slice

_cap_cgp := $(if $(strip $(CAP_CGROUP_PARENT)),--cgroup-parent=$(CAP_CGROUP_PARENT),)

# podman BUILD accepts --memory/--memory-swap/--cgroup-parent (NOT --cpus/--pids-limit)
PODMAN_BUILD_CAPS := --memory=$(CAP_MEM) --memory-swap=$(CAP_SWAP) $(_cap_cgp)
# podman RUN accepts the full set
PODMAN_RUN_CAPS   := --memory=$(CAP_MEM) --memory-swap=$(CAP_SWAP) \
                     --pids-limit=$(CAP_PIDS) --cpus=$(CAP_CPUS) $(_cap_cgp)

.PHONY: image build help fmt fmt-check check clean test-tauri

.DEFAULT_GOAL := help

# ADR-0019 Tier-1 verbs: help build test lint fmt check clean. `build` (alias of
# `wasm`), `test`, `lint` already exist below; help/fmt/check/clean are added
# here. Every recipe runs inside the toolchain image (host needs only make+podman).
help:
	@echo "entity-browser-rust — make + podman (host needs only make + podman)"
	@echo
	@echo "  build      WASM debug build → dist/ (alias of wasm; conventional entry)"
	@echo "  test       main-crate unit + peer-integration suite, in-container"
	@echo "  test-tauri src-tauri backend unit tests (workspace-excluded from test)"
	@echo "  lint       cargo clippy, in-container (read-only)"
	@echo "  fmt        cargo fmt, in-container (writes) · fmt-check verifies only"
	@echo "  check      lint + test + test-tauri (the green gate)"
	@echo "  clean      remove build outputs + run artifacts + the toolchain image"
	@echo
	@echo "  wasm / wasm-release / serve / build-serve / e2e-worker"
	@echo "    e2e-worker  T=<test> UNTIL=<phase> SKIP_BUILD=1 narrow the run"
	@echo "    e2e-phases  list what T= and UNTIL= accept"
	@echo "    e2e-webrtc  two-browser §6.5 WebRTC S5 gate (host podman; BUILD=1 rebuilds dist/)"
	@echo "    e2e-webrtc-nat  NEGATIVE control: isolated networks, media must NOT cross (the ICE gap)"
	@echo "  desktop (containerized display passthrough — Wayland/X11):"
	@echo "    tauri-run            run the app; HOST_HOME=1 uses your real \$$HOME,"
	@echo "                         SHARE_DIR=<dir> shares a host folder over local/files"
	@echo "    host-run             run the container-built binary NATIVE (mutable hosts only)"
	@echo "    appimage             portable self-contained AppImage (release artifact)"
	@echo "  release: wasm-release · publish · tauri-bundle (see docs/RELEASE-READINESS.md)"
	@echo "  — see the Makefile header for the full target catalogue."

# Build the toolchain image (rust 1.94.1 + wasm32 + trunk + binaryen + webkit2gtk).
image:
	podman build $(PODMAN_BUILD_CAPS) -t $(IMAGE) .

# Run a command inside the toolchain image with the parent meta dir mounted at
# /src/entity-systems (sibling path-deps resolve) and workdir = this repo. The
# cargo registry/cache is a persistent volume so deps aren't re-downloaded every
# build. Resource caps (PODMAN_RUN_CAPS) bound every container.
define RUN
	mkdir -p $(CARGO_CACHE) $(TRUNK_CACHE)
	podman run --rm $(PODMAN_RUN_CAPS) $(2) \
		-v $(PARENT):/src/entity-systems:z \
		-v $(CARGO_CACHE):/usr/local/cargo/registry:z \
		-v $(TRUNK_CACHE):/root/.cache:z \
		-e CARGO_TARGET_DIR=$(TARGET_DIR) \
		$(EXTRA_RUN_ENV) \
		-w /src/entity-systems/$(notdir $(CURDIR)) \
		$(IMAGE) \
		sh -c '$(1)'
endef

# Serve a static directory ($(1)) from inside the image, on $(PORT). Keeps the
# "podman + make only" contract — no host python3. $(1) is resolved relative to
# the workdir (this repo) OR may be an absolute in-container path (e.g. a mount
# supplied via the $(2) extra-flags param — see publish-serve's SERVE_DIR).
# `--network host`: the container binds the host port directly (rootless `-p`
# port-forwarding resets connections under pasta/slirp; host-net is reliable and
# is what a local dev server wants). Foreground; Ctrl-C stops it.
define RUN_SERVE
	podman run --rm $(PODMAN_RUN_CAPS) --network host $(2) \
		-v $(PARENT):/src/entity-systems:z \
		-w /src/entity-systems/$(notdir $(CURDIR)) \
		$(IMAGE) \
		python3 -m http.server $(PORT) --bind 0.0.0.0 --directory $(1)
endef

# Repo-local, gitignored HOME for the containerized desktop app so its durable
# config (`~/.local/share/<app-id>`) persists across runs. `rm -rf` it for a
# fresh cold-boot profile (the returning-app durable-config-wins gotcha).
# Because HOME is a host bind-mount, the backend's file-share root
# (`$HOME/.entity/tori-share`, src-tauri/src/lib.rs `ensure_share_root`) is
# ALREADY host-visible+persistent at `$(TAURI_HOME)/.entity/tori-share` — files
# pulled/pushed over `local/files` land on the host, not in a container layer.
TAURI_HOME ?= .tauri-home

# Optional: expose an ARBITRARY host directory as the file-transfer share root.
# `make tauri-run SHARE_DIR=~/Downloads` bind-mounts that host dir onto the
# backend's `local/files/shared` root, so the desktop app transfers files
# straight to/from a real host folder (a USB mount, a project dir, …). Empty =
# the default repo-local `$(TAURI_HOME)/.entity/tori-share`. (label=disable +
# keep-id give the container access; no SELinux relabel of your dir.)
SHARE_DIR ?=

# Optional: run against your REAL home directory instead of the isolated
# `$(TAURI_HOME)` profile. `make tauri-run HOST_HOME=1` mounts your actual
# `$(HOME)` as the app's home (uid already matches via keep-id), so it uses your
# real config and the file-transfer share is your real `~/.entity/tori-share` —
# filesystem-identical to a bare-native run. This is the answer on an IMMUTABLE
# distro (Silverblue/Kinoite), where you can't install the webkit lib to run the
# binary natively: the container just supplies that one lib (exactly like a
# Flatpak does) while HOST_HOME=1 gives it your real filesystem — same binary,
# same kernel, same home. Broader FS access than the isolated default, so it is
# opt-in. (Takes precedence over SHARE_DIR — the share is already your real home.)
HOST_HOME ?=

# Launch a WebKitGTK/GTK binary ($(1)) from INSIDE the image onto the host's
# display — so `make tauri-run` needs only make + podman, NOT a host webkit or
# a host toolchain (webkit2gtk lives in the image; the build already links it).
#
# Display passthrough is Wayland-primary (the proven path on a Wayland session)
# and falls back to X11/XWayland. Load-bearing flags:
# - `--security-opt label=disable` — Fedora SELinux otherwise blocks the
#   container from opening the host compositor socket (it shows up `-?????`).
# - `--userns=keep-id` — map the invoking user 1:1 so the socket's uid matches.
# - `WEBKIT_DISABLE_DMABUF_RENDERER=1` (+ compositing off) — WebKitGTK's GPU
#   dmabuf path is unreliable inside a container; force the software compositor.
# - `--device /dev/dri` when present (harmless extra; software GL needs no GPU).
# Foreground; close the window or Ctrl-C to stop. Needs a display — headless
# hosts can't present a window (there is nothing to containerize about that).
define RUN_GUI
	@mkdir -p $(TAURI_HOME) $(SHARE_DIR)
	@set -e; disp=""; dri=""; \
	if [ -n "$$WAYLAND_DISPLAY" ] && [ -S "$$XDG_RUNTIME_DIR/$$WAYLAND_DISPLAY" ]; then \
	  disp="-e XDG_RUNTIME_DIR=/tmp/xdg -e WAYLAND_DISPLAY=$$WAYLAND_DISPLAY -e GDK_BACKEND=wayland -v $$XDG_RUNTIME_DIR/$$WAYLAND_DISPLAY:/tmp/xdg/$$WAYLAND_DISPLAY"; \
	  echo "==> launching on Wayland ($$WAYLAND_DISPLAY) inside $(IMAGE)"; \
	elif [ -n "$$DISPLAY" ]; then \
	  xhost +local: >/dev/null 2>&1 || true; \
	  disp="-e DISPLAY=$$DISPLAY -e GDK_BACKEND=x11 -v /tmp/.X11-unix:/tmp/.X11-unix"; \
	  [ -n "$$XAUTHORITY" ] && disp="$$disp -e XAUTHORITY=/tmp/.Xauth -v $$XAUTHORITY:/tmp/.Xauth:ro"; \
	  echo "==> launching on X11 ($$DISPLAY) inside $(IMAGE)"; \
	else \
	  echo "no WAYLAND_DISPLAY or DISPLAY set — no display to present a window on"; exit 1; \
	fi; \
	[ -d /dev/dri ] && dri="--device /dev/dri"; \
	podman run --rm $(PODMAN_RUN_CAPS) --userns=keep-id --security-opt label=disable \
	  $$disp $$dri --net=host \
	  -e WEBKIT_DISABLE_DMABUF_RENDERER=1 -e WEBKIT_DISABLE_COMPOSITING_MODE=1 \
	  $(if $(HOST_HOME),\
	    -e HOME=$(HOME) -e XDG_CACHE_HOME=$(HOME)/.cache -v $(HOME):$(HOME),\
	    -e HOME=/tmp/tauri-home -e XDG_CACHE_HOME=/tmp/tauri-home/.cache -v $(CURDIR)/$(TAURI_HOME):/tmp/tauri-home $(if $(SHARE_DIR),-v $(abspath $(SHARE_DIR)):/tmp/tauri-home/.entity/tori-share,)) \
	  -v $(PARENT):/src/entity-systems:z \
	  -w /src/entity-systems/$(notdir $(CURDIR)) \
	  $(IMAGE) \
	  $(1)
endef

# There is no native UI build. The native binary is a deprecation stub
# that prints a redirect to the active targets. (The legacy eframe
# renderer was removed.)
native:
	@echo "make native is deprecated — there is no native UI build."
	@echo ""
	@echo "  make wasm         — browser build (DOM)"
	@echo "  make tauri-run    — desktop build (DOM in WebView + native backend peer)"
	@echo ""
	@exit 1

# Run tests (native unit + peer-integration), in-container. The
# Selenium-dependent e2e suite is gated behind the `e2e` cargo feature
# (off by default), so this target is fully bare-box: no Selenium needed.
# Run `make e2e-worker` for the browser E2E path.
test: image
	$(call RUN,cargo test)

# The desktop backend (src-tauri) is workspace-EXCLUDED (Cargo.toml `exclude`),
# so `make test` does NOT run its unit tests — including the authorize-gate
# manager-grant seeding (manager_grant.rs) and persistence.rs. Run them here,
# in-container (compiles src-tauri, which links webkit2gtk from the image).
test-tauri: image
	$(call RUN,cd src-tauri && cargo test)

# Lint, in-container: clippy + the UI ratchet gate (raw atoms / inline style
# literals / untokenized hex must match tools/ui-lint-baseline.txt — see
# tools/ui-lint.sh; migrations ratchet the baseline down in the same commit) +
# the i18n ratchet gate (raw UI-text literals in anchored positions must match
# tools/i18n-lint-baseline.txt — see tools/i18n-lint.sh; string migrations
# ratchet it down in the same commit) + the two i18n correctness gates:
# i18n_locale_check (catalogs vs the EN base) and i18n_callsite_check (every
# literal t("key") resolves, and passes the slots its template interpolates —
# a missing key renders the RAW KEY to the user, and keys are strings, so
# nothing else catches a rename that misses a call site).
lint: image
	$(call RUN,cargo clippy && ./tools/ui-lint.sh && ./tools/i18n-lint.sh && python3 tools/i18n_locale_check.py && python3 tools/i18n_callsite_check.py)

# Tier-1 fmt = autoformat (writes), in-container.
fmt: image
	$(call RUN,cargo fmt)

# Format verification (read-only) — the gate variant of `fmt`. Fails if any
# file isn't formatted, without writing. Not in `check` by default (kept fast
# + non-surprising); run in CI / before a release.
fmt-check: image
	$(call RUN,cargo fmt --check)

# Tier-1 check = the green gate: lint + BOTH test suites (main crate + the
# workspace-excluded src-tauri backend).
check: lint test test-tauri

# Tier-1 clean = remove the host-visible build OUTPUTS + run artifacts and the
# toolchain image. Leaves the expensive COMPILE CACHES intact (target/,
# target-publish/, src-tauri/target/ objects, and the cargo registry named
# volume $(CARGO_CACHE)) — `rm -rf` those by hand for a full cold reset.
# Cleaned: SPA bundles (dist*), the desktop app's isolated profile+cache
# (.tauri-home, .cache), and the AppImage/bundle output.
clean:
	rm -rf dist/ dist-publish/ .tauri-home/ .cache/ src-tauri/target/release/bundle/
	-podman rmi $(IMAGE)

# DEMO bakes the `demo-apps` fixtures (War, Calculator, and the L5 demos **Ping**
# + **Life**) into the debug build so a plain `make serve` / `make tauri-run` can
# render + launch them without a live origin — the fast way to actually *use* the
# L5 app-host (open the Apps window → "Life (L5)"). **ON by default** for now (dev
# convenience); `make wasm DEMO=0` turns it off. Only the DEBUG `wasm` target is
# affected — `wasm-release` / `build-serve` stay clean, so the SHIPPING artifact
# never bakes fixtures (real deployments serve apps off a registered origin). When
# we bundle/ship for real we revisit this default. (Same feature the e2e uses; see
# views/games ensure_demo_set.)
DEMO ?= 1
WASM_FEATURES ?=
ifeq ($(DEMO),1)
WASM_FEATURES += --features demo-apps
endif

# WASM debug build → dist/. Single bundle for both Direct (default)
# and Worker (`?worker=1`) modes — capability detection at boot picks
# Worker automatically when available and falls back to Direct on
# failure (Stage 1B). `DEMO=1` bakes the demo apps (incl. the L5 demos).
wasm: image
	$(call RUN,trunk build $(WASM_FEATURES) --dist $(DIST) && ./tools/check-dist.sh $(DIST))

# Alias — `make build` is the conventional bare-box entry point across the repo group.
build: wasm

# WASM release build → dist/
wasm-release: image
	$(call RUN,trunk build --release --dist $(DIST) && ./tools/check-dist.sh $(DIST))

# Execute the upstream wasm-worker-protocol crate's `#[wasm_bindgen_test]`
# suites (v11_wire_shape, …). That crate is `#![cfg(target_arch = "wasm32")]`,
# so `make test` cannot see these — they need a wasm runner. Charter bars Node
# from the build toolchain, but an EPHEMERAL container may pull one purely to
# run a test: this fetches node 22 (18 is too old for wasm-bindgen's GC glue —
# see the script) + the version-matched runner, writes only to /tmp. Needs
# network. Not part of the `make lint`/`make test` gate; run it when the wire
# shape changes. Verified 2026-08-03: v11_wire_shape 8/8 green.
wasm-test-protocol: image
	$(call RUN,sh tools/e2e/run-protocol-wasm-tests.sh)

# Deploy-staleness guard: verify dist/ is internally consistent (index.html
# references only bundles that exist + are non-empty). Catches the class
# `make e2e-worker` can't — see the SW-cache-and-durability review.
check-dist:
	@./tools/check-dist.sh

# Phase 0c — frame-time measurement build. Same as `wasm` but with the
# `measurement` cargo feature, which enables per-frame L0-call counters
# logged to the browser console (src/frame_counters.rs). Use with
# `make serve` and a representative working session to gather counts
# before Phase 1 freezes the cache API.
wasm-measurement: image
	$(call RUN,trunk build --features measurement --dist $(DIST))

# Display passthrough for the e2e's Tauri phases (14 / 15.6): the desktop
# binary needs a window server or GTK dies at init, so hand the container
# the host's display socket when one exists — same plumbing as RUN_GUI,
# minus --userns=keep-id (the cargo caches in the image are root-owned, and
# rootless podman already maps container-root to the invoking user, so the
# socket is accessible without it — verified on this Wayland host). With no
# display this expands empty and the Tauri phases self-skip LOUDLY
# (display-gated in tests/e2e_worker.rs) instead of failing the suite red;
# the port is never an issue either way (the listener falls back to a
# dynamic port when 4041 is taken, and the test reads ws_addr from the
# READY line).
E2E_DISPLAY_ARGS = $(shell \
  if [ -n "$$WAYLAND_DISPLAY" ] && [ -S "$$XDG_RUNTIME_DIR/$$WAYLAND_DISPLAY" ]; then \
    echo "--security-opt label=disable -e XDG_RUNTIME_DIR=/tmp/xdg -e WAYLAND_DISPLAY=$$WAYLAND_DISPLAY -e GDK_BACKEND=wayland -v $$XDG_RUNTIME_DIR/$$WAYLAND_DISPLAY:/tmp/xdg/$$WAYLAND_DISPLAY $(if $(wildcard /dev/dri),--device /dev/dri,) -e WEBKIT_DISABLE_DMABUF_RENDERER=1 -e WEBKIT_DISABLE_COMPOSITING_MODE=1"; \
  elif [ -n "$$DISPLAY" ]; then \
    echo "--security-opt label=disable -e DISPLAY=$$DISPLAY -e GDK_BACKEND=x11 -v /tmp/.X11-unix:/tmp/.X11-unix $(if $(wildcard /dev/dri),--device /dev/dri,) -e WEBKIT_DISABLE_DMABUF_RENDERER=1 -e WEBKIT_DISABLE_COMPOSITING_MODE=1"; \
  fi)

# E2E browser test (exercises Worker mode via `?worker=1`). Requires
# the Selenium-firefox container running on :4444 — see
# tools/e2e/README.md. The test prints the full captured browser
# console under --nocapture, so this is the primary diagnostic path
# without manual browser refresh.
#
# Narrowing the run (full suite is ~4.5 min; see tools/e2e/README.md §Filtering):
#   make e2e-worker T=frontend_idb        # only tests whose name contains this
#   make e2e-worker UNTIL=20              # monolith stops after Phase 20
#   make e2e-worker SKIP_BUILD=1          # reuse dist/ — skip the trunk rebuild
# `make e2e-phases` lists the phase labels UNTIL accepts.
E2E_UNTIL_ENV = $(if $(strip $(UNTIL)),-e E2E_UNTIL=$(strip $(UNTIL)),)

# Hard wall-clock cap on an e2e run (see the `timeout` note in e2e-worker).
# A healthy full suite is ~285s; 15m is ~3x headroom for a loaded box, so it
# only ever fires on a genuine hang. Override: `make e2e-worker E2E_TIMEOUT=25m`.
E2E_TIMEOUT ?= 15m
# Preflight: the suite's own connect error is good, but it only surfaces on the
# far side of the trunk build — a minute burnt on the commonest mistake. Probe
# :4444 first (in-container python3, so the host still needs only make+podman).
E2E_PREFLIGHT = python3 -c "import urllib.request as u; u.urlopen(\"http://localhost:4444/status\", timeout=3)" 2>/dev/null \
	|| { echo; echo "e2e preflight: nothing answering on :4444 — start Selenium first:"; \
	     echo "  podman run -d --rm --name e2e-firefox --network=host docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404"; \
	     echo "  (details: tools/e2e/README.md)"; echo; exit 1; }
e2e-worker: image
	@$(call RUN,$(E2E_PREFLIGHT),--network host)
	# The e2e dist is built WITH `--features demo-apps`: the launcher→player
	# e2e (Phase 2h.2) needs a deterministic baked app (war/calculator) to
	# render + launch without a live origin. demo-apps is OFF in every
	# release/serve/tauri build — no fake apps are shipped (see Cargo.toml
	# [features]) — so this is the ONE build that bakes them.
	#
	# SKIP_BUILD=1 reuses whatever is in dist/. It is a DEV shortcut only: if
	# dist/ was last built by `make wasm` it has NO demo-apps, and Phase 2h.2
	# fails for that reason and not a real one. Never use it for a gate run.
ifeq ($(strip $(SKIP_BUILD)),)
	$(call RUN,trunk build --features demo-apps --dist $(DIST) && ./tools/check-dist.sh $(DIST))
else
	@echo ">>> SKIP_BUILD=1 — reusing the existing $(DIST)/ (NOT a gate-grade run)"
	@./tools/check-dist.sh $(DIST)
endif
	# --features e2e: the e2e_worker suite is `#![cfg(feature = "e2e")]`, so it
	# compiles to nothing (and `make test` stays bare-box green) UNLESS the
	# feature is on. This target turns it on; it needs Selenium on :4444.
	# --test-threads=1: the e2e tests share one Selenium session + http port,
	# so they must run serially (the main boot test + the multi-tab guard test).
	# In-container (podman+make only); --network host so the test reaches the
	# Selenium container on :4444 and Selenium reaches the test's :8092 server.
	# $(T) is cargo's own substring filter over test names; UNTIL rides in as
	# E2E_UNTIL and cuts the monolith short (see tests/e2e_worker.rs PHASE_ORDER).
	#
	# `timeout` is the OUTERMOST of three guards, and the only one that survives
	# a wedge in the layers below it (cargo itself, the container, a hung
	# socket). Inside it: the suite's stall watchdog (E2E_STALL_SECS, names the
	# stuck phase — the diagnosis you actually want) and the WebDriver
	# script/pageLoad timeouts. A healthy full run is ~285s, so E2E_TIMEOUT
	# never fires on one; it exists so a hang FAILS instead of sitting silent
	# forever in CI or an agent loop. --signal=KILL because a wedged podman
	# child may not honour TERM.
	$(call RUN,timeout --signal=KILL $(E2E_TIMEOUT) cargo test --features e2e --test e2e_worker $(strip $(T)) -- --nocapture --test-threads=1,--network host $(E2E_DISPLAY_ARGS) $(E2E_UNTIL_ENV))

# List what `T=` and `UNTIL=` accept. Reads the test source, so it can never
# drift from what actually runs — and needs neither the image nor Selenium.
e2e-phases:
	@echo "T=<substring of a test name> — the independent tests:"
	@awk '/^#\[tokio::test/{f=1;next} f&&/^async fn/{sub(/\(.*/,"");print "  " $$3;f=0}' tests/e2e_worker.rs
	@echo
	@echo "UNTIL=<phase> — phases of worker_boots_and_opens_all_windows, in run order:"
	@sed -n '/^const PHASE_ORDER/,/^];/p' tests/e2e_worker.rs | sed -e '1d' -e '$$d' -e 's/^/ /'

# The two-browser §6.5 WebRTC gate — the terminal S5 validation. Unlike
# `e2e-worker` (ONE Selenium session on :4444), this stands up TWO firefox
# containers on a shared podman bridge + a real signaling node: the only
# topology in which browser↔browser ICE succeeds. Rootless pasta mirrors the
# host IP into a default-network container, so a host-net pair advertises
# colliding candidates and ICE fails; the bridge gives distinct routable IPs.
# See tools/e2e/webrtc-rung1/README.md + the "WebRTC / two-browser" gotcha in
# AGENTS.md.
#
# It orchestrates podman DIRECTLY (creates a network, runs containers, rebuilds
# the signaling node from core-rust HEAD to defeat build-skew), so it runs on
# the HOST, not via $(call RUN) — podman-in-podman would defeat the point. Host
# needs make + podman + python3 (python3 already drives the existing e2e).
#
# Fail-closed: the rig's BIDIRECTIONAL verdict (both channels open AND both
# directions status=200) is the exit code. Containers/node/dist-server are torn
# down before (clean slate) and after (cleanup) regardless of outcome.
#
# Requires dist/ (build once with `make wasm`); `BUILD=1` rebuilds it here.
e2e-webrtc:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc SKIPPED: podman not found on host (this gate manages containers directly)"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc: two-browser §6.5 WebRTC S5 gate (host podman)"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc: PASS"; else echo ">>> e2e-webrtc: FAIL (rc=$$rc)"; fi; \
	 exit $$rc

# Two-browser CHAT over WebRTC — the app/chat full-flow live gate. Reuses the
# same infra as e2e-webrtc (bridge + 2 firefox + signaling node + dist), but
# drives the chat spike: both peers open Chat, bind the 1:1 (peer-id input), and
# a message must cross the data channel BOTH ways. Proves delivery is transport-
# agnostic (worker-mode WebRTC via ChatDelivery's poll path).
e2e-webrtc-chat:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-chat SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-chat BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-chat: two-browser chat over §6.5 WebRTC (host podman)"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; SPIKE=spike_chat_over_webrtc.py SPIKE_ARGS="" bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-chat: PASS"; else echo ">>> e2e-webrtc-chat: FAIL (rc=$$rc)"; fi; \
	 exit $$rc

# MEET THEN CHAT — the whole product claim: name -> id -> connection -> message.
# Same infra again, but nothing is handed to the browsers in the URL: they add the
# signaling node to their own connector registry through the Shell, reload (so
# provisioning resolves from that choice — InitParams.webrtc is Init-only), both
# `meet tag <label>`, and chat over the id each one LEARNED. This is the gate that
# covers the shipped path; e2e-webrtc-chat covers the mechanism with the node, the
# peer id and `?webrtc_enable=1` all supplied by the harness.
#
# GREEN. It was not, for most of a session, and the cause was entirely in the
# harness: it submitted each Shell command once, immediately after clicking
# "+ Shell". That click only QUEUES an action — the window is created on the next
# frame — so the first attempt found no input element and typed nothing, then
# reported the command as ignored. It read as a post-reload app freeze because the
# pre-reload Shell had already been opened and typed into. Every interaction now
# retries until the app visibly responds, waits for the window before typing, and
# reads the app's own output for confirmation; the spike's docstring has the
# measured trace and what was ruled out on the way.
e2e-webrtc-meet:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-meet SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-meet BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-meet: two browsers meet at a name, then chat (host podman)"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; SPIKE=spike_meet_then_chat.py SPIKE_ARGS="" bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-meet: PASS"; else echo ">>> e2e-webrtc-meet: FAIL (rc=$$rc)"; fi; \
	 exit $$rc

# The NEGATIVE control: the same two browsers, but on ISOLATED networks with no
# route between them — what "two peers behind different NATs" looks like to the
# app. Rendezvous still works (both reach the node through the host); the data
# channel must NOT open, because `resolve_webrtc_provisioning` hardcodes
# `ice_servers: Vec::new()` and host candidates cannot cross.
#
# PASSING MEANS THE CONNECTION FAILED, on purpose. This is the instrument that
# measures the ICE gap rather than reasoning about it, and it is the standing
# answer to "e2e-webrtc-meet is green, so we work on the internet" — that gate
# runs on ONE subnet, where host candidates always work.
#
# When ICE servers land (EXTENSION-SIGNALING §4.5.1 reflection endpoints), flip
# EXPECT_NO_MEDIA off and this same rig becomes the positive NAT-traversal gate.
e2e-webrtc-nat:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-nat SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-nat BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-nat: NEGATIVE control — two isolated networks, media must NOT cross"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; TOPOLOGY=split EXPECT_NO_MEDIA=1 SPIKE=spike_meet_then_chat.py SPIKE_ARGS="" \
	   bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-nat: PASS (discovery works, media does not — the ICE gap, measured)"; \
	 else echo ">>> e2e-webrtc-nat: FAIL (rc=$$rc) — read the RESULT line above; media crossing is the interesting case"; fi; \
	 exit $$rc

# Tauri desktop (DEBUG WASM by default + debug backend, logs to stdout).
# Use this for development — the fast dev loop. Debug WASM is STABLE in Tauri:
# the `[profile.dev.package.*]` overrides in Cargo.toml (curve25519-dalek,
# ed25519-dalek, sha2, block-buffer → overflow-checks=false) already neutralize
# the only debug-build panics, so there is NO reason to pay the release tax to
# just run the app. The old default was `wasm-release` (fat LTO + codegen-
# units=1 + wasm-opt -Oz) — three whole-module, non-incremental passes that
# re-run from scratch every launch (caching can't touch them), which is why it
# took "forever". Those belong to `tauri-bundle` (shipping size), not the dev
# loop. Want the size-optimized artifact here anyway? `make tauri-run
# TAURI_WASM=wasm-release`. (With debug WASM the WebKit Inspector source-maps
# Rust again; the release profile strips that.)
#
# NOTE: Same unified bundle as `make wasm`; runtime boot detects Tauri
# and forces Direct mode (src/main.rs). WebKitGTK ≤ 2.52 lacks
# `WorkerNavigator.storage`, so worker-mode OPFS init HARD-FAILS there
# (it does not silently in-memory — build_async returns Err, which would
# round-trip and fall back to Direct+banner). We preemptively force Direct
# in Tauri to skip that wasted failed-worker spawn. Browser deployments get
# worker-mode automatically via capability detection. Tracked in
# WORKER-MODE-LIVING-DOC §3.6.
TAURI_WASM ?= wasm
tauri: $(TAURI_WASM)
	# Re-embed the freshly-built frontend. `tauri::generate_context!()`
	# (src-tauri/src/lib.rs) reads ../dist at macro-expansion time, but
	# cargo's incremental compiler can't see that dependency — so when only
	# dist/ changed it leaves lib.rs "up to date" and embeds the STALE
	# frontend (you launch an old UI). We bypass Tauri's CLI asset pipeline
	# here (raw `cargo build`), so nothing else tracks it. Touching the
	# embedding source forces a re-expand + re-embed every build. Cheap:
	# the $(TAURI_WASM) prerequisite reran the frontend build anyway (trunk
	# rewrites dist/), so this is never a wasted no-op.
	$(call RUN,touch src-tauri/src/lib.rs && cd src-tauri && cargo build)
	@echo ""
	@echo "Built: ./src-tauri/target/debug/entity-browser-tauri"
	@echo "Launch (containerized, needs a display on the host): make tauri-run"
	@echo "Inspector: right-click → Inspect Element in the WebView"

# Tauri — build and run in one step, CONTAINERIZED with display passthrough.
# Host needs only make + podman (webkit2gtk lives in the image); it does need a
# display to present the window (Wayland or X11). rm -rf .tauri-home for a fresh
# cold-boot profile. See the RUN_GUI macro for the passthrough mechanics.
tauri-run: tauri
	$(call RUN_GUI,./src-tauri/target/debug/entity-browser-tauri)

# Build in the container, RUN NATIVELY on the host — the conventional desktop
# model, and the SIMPLE one: the container-built binary is a normal host process,
# so NONE of the containerization-at-runtime plumbing applies. Your REAL $HOME,
# your real filesystem (the file-transfer share is just `~/.entity/tori-share`
# in your actual home — no SHARE_DIR mount), the native display (no passthrough,
# no SELinux flag, no keep-id). The build stays fully containerized (no host
# cargo/rust); the ONLY host runtime dep is the WebKitGTK library the binary
# dynamically links — a GUI runtime lib, not a build toolchain. Missing it? this
# prints how to get it and stops. NOTE: on an IMMUTABLE distro (Silverblue/
# Kinoite) you can't layer a lib onto the host cheaply — there, don't use this;
# use `make tauri-run HOST_HOME=1` (the container supplies webkit like a Flatpak,
# HOST_HOME gives it your real filesystem). This target is for mutable hosts.
host-run: tauri
	@if ! ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1.so.0; then \
	  echo "native run needs the WebKitGTK 4.1 runtime the binary links against."; \
	  if [ -f /run/ostree-booted ]; then \
	    echo "  This is an IMMUTABLE (ostree) host — don't layer libs onto it. Instead:"; \
	    echo "      make tauri-run HOST_HOME=1   # container carries webkit; runs on your real \$$HOME"; \
	  else \
	    echo "    sudo dnf install webkit2gtk4.1            # Fedora"; \
	    echo "    sudo apt install libwebkit2gtk-4.1-0      # Debian / Ubuntu"; \
	    echo "  (everything else it needs — gtk3, libsoup3 — is already present here.)"; \
	    echo "  Or run fully containerized instead: make tauri-run"; \
	  fi; \
	  exit 1; \
	fi
	@echo "==> running the container-built binary NATIVELY on the host"
	./src-tauri/target/debug/entity-browser-tauri

# Build a portable **AppImage** — a single self-contained file bundling the
# binary + webkit2gtk + every runtime lib (Tauri's bundler). This is the
# RELEASE-distribution artifact: it runs on other Linux hosts with NO dev
# toolchain (no podman, no make, no host webkit). Built fully in-container
# (`APPIMAGE_EXTRACT_AND_RUN=1` because the container has no FUSE; the bundler
# downloads linuxdeploy/appimagetool). `touch lib.rs` forces a fresh dist/
# re-embed (same reason as `make tauri`). First cut for release builds — see
# docs/RELEASE-READINESS.md when we harden it (icon set, signing, deb/rpm).
# NOTE: to *run* an AppImage the target host needs FUSE; on a FUSE-less host
# (this immutable box included) launch it with `--appimage-extract-and-run`.
appimage: wasm-release
	$(call RUN,touch src-tauri/src/lib.rs && cd src-tauri && APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=1 cargo tauri build --bundles appimage)
	@echo ""
	@echo "=== portable AppImage(s) built ==="
	@find src-tauri/target -name '*.AppImage' -exec ls -lh {} \; 2>/dev/null | awk '{print "  " $$NF "  (" $$5 ")"}'
	@echo "  run on a FUSE host:      ./<name>.AppImage"
	@echo "  run on a FUSE-less host: ./<name>.AppImage --appimage-extract-and-run"

# Content-baked desktop bundle: embed the SPA **plus published sites/apps** into
# the Tauri binary, served same-origin from the WebView's asset protocol (no
# server, fully offline). The ordering is load-bearing and is exactly why plain
# `make tauri` can't do it: wasm-release FIRST (trunk WIPES dist/), THEN publish
# INTO dist/ (cleans only its own roots — sites/content/{peer} — leaving the SPA),
# THEN embed. Same publish knobs as `make publish`
# (INGEST / APPS_DIST / CONFIG_SITE / SURFACE / WINDOW_TYPE / LOCKED / IDENTITY_SEED);
# --deployment-config is implied (a baked bundle must boot into its content).
#
# INGEST / APPS_DIST may point ANYWHERE (absolute or relative) — the recipe stages
# them into repo-local dirs first, because the publish runs in a container that
# only bind-mounts this repo (so an external path isn't visible inside). You do
# NOT copy anything by hand:
#   make tauri-bundle-run APPS_DIST=../../entity-systems/entity-apps/dist CONFIG_SITE=demo
#
# NOTE: a returning desktop app keeps its DURABLE config, which wins over the
# baked deployment config (persisted > fetched > build-time). A fresh install
# boots into the baked content; to re-test, clear ~/.local/share/<app-identifier>.
APPS_STAGE   := .apps-stage
INGEST_STAGE := .ingest-stage
tauri-bundle: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
tauri-bundle: wasm-release
	@mkdir -p $(PUBLISH_DATA_DIR)
	@rm -rf $(APPS_STAGE) $(INGEST_STAGE)
	$(if $(APPS_DIST),cp -r $(APPS_DIST) $(APPS_STAGE))
	$(if $(INGEST),cp -r $(INGEST) $(INGEST_STAGE))
	$(call RUN,cargo run --quiet --bin entity-browser -- publish dist $(if $(INGEST),--ingest=$(INGEST_STAGE),) $(if $(APPS_DIST),--ingest-apps=$(APPS_STAGE),) $(if $(LIVE),--live=$(LIVE),) --deployment-config $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,))
	@rm -rf $(APPS_STAGE) $(INGEST_STAGE)
	$(call RUN,touch src-tauri/src/lib.rs && cd src-tauri && cargo build)
	@echo ""
	@echo "Built content-baked desktop app: ./src-tauri/target/debug/entity-browser-tauri"
	@echo "Launch: make tauri-bundle-run  (or run the binary directly on a desktop session)"

tauri-bundle-run: tauri-bundle
	$(call RUN_GUI,./src-tauri/target/debug/entity-browser-tauri)

# Regenerate the compute-program fixture bundles (assets/programs/*.json)
# by running the UNCHANGED workbench-go authoring code in the pinned golang
# image and dumping the authored entities + per-tick oracle hashes
# (tools/program-dump; DESIGN-COMPUTE-PROGRAM-HOST-POC.md). Go is a
# fixture-generation dependency only — never part of the app toolchain.
# Needs the sibling checkouts ../entity-workbench-go and ../entity-core-go.
program-fixtures:
	mkdir -p assets/programs
	podman run --rm \
		-v $(PARENT):/src:z \
		-v program-dump-gocache:/go/pkg/mod \
		-w /src/$(notdir $(CURDIR))/tools/program-dump \
		golang:1.25-bookworm \
		sh -c "go mod tidy && go run . -out ../../assets/programs"

# Serve whatever is currently in dist/ (no rebuild). Fast, but does NOT
# guarantee the bundle is current — use `make build-serve` when you need
# certainty you're serving the latest optimized build.
serve:
	@echo "  → http://localhost:$(PORT)   (override with: make serve PORT=8082)"
	$(call RUN_SERVE,dist)

# Build WITH the demo apps (incl. the L5 Ping + Life demos) and serve — the
# one-command way to actually try L5 app-hosting in a real browser. Open the
# Apps window → "Life (L5)" to watch Conway's Life run in a WASM entity-peer
# inside a same-origin sandboxed iframe. (The demos are on by default now, so a
# plain `make serve` after any build works too; this just guarantees a fresh one.)
serve-demo: image
	$(call RUN,trunk build --features demo-apps --dist $(DIST) && ./tools/check-dist.sh $(DIST))
	@echo ""
	@echo "  → http://localhost:$(PORT)   — open the Apps window → 'Life (L5)'"
	@echo ""
	$(call RUN_SERVE,dist)

# Build the SHIPPING (release-optimized) WASM, then serve it — always the
# latest. Use this when you want certainty you're running the real
# optimized release artifact (`make serve` alone does NOT rebuild). Prints
# the bundle sizes so you can see it's the optimized build. Slower than
# `make serve` because the release profile is opt-level=z + fat LTO.
build-serve: wasm-release
	@echo ""
	@echo "=== SHIPPING (release-optimized) build — serving latest ==="
	@ls -lh dist/*.wasm 2>/dev/null | awk '{print "  " $$9 "  " $$5}'
	@echo "  → http://localhost:$(PORT)   (override with: make build-serve PORT=8082)"
	@echo ""
	$(call RUN_SERVE,dist)

# Publish — render the site set to static no-JS HTML (the legacy-web /
# CDN / permalink projection). Headless native, no browser: builds a peer,
# reads its sites off the tree ([A] reader), projects them to
# `dist/static-demo/sites/{peer}/{site}/…` ([B1] emitter). Override the
# output dir: `make publish OUT=path/to/dir`. OUT must stay UNDER the repo tree
# (the default is repo-relative): publish runs in-container with only the parent
# meta dir bind-mounted, so an absolute OUT like `/tmp/x` writes to the
# container's throwaway /tmp and the result never reaches the host. Today the source is a fresh
# peer seeded with the demo site set (a demo/SSG generator) — publishing a
# durable dedicated hosting peer's real tree is the deferred peer-source
# seam (src/content_site/publish.rs). Serve the result with `make serve`
# (open /static-demo/sites/<peer>/) or the printed python3 one-liner.
# Emits BOTH forms into one dir: legacy-web `.html` (sites/{peer}/…, no-JS)
# AND entity-native `.bin` content data (content/… + {peer}/sites/…, what a
# live peer ingests). Sites-scoped — never the whole peer tree.
#   INGEST=<dir>    source sites from an on-disk ingest directory (disk→tree) —
#                   any generator's output OR a hand-authored folder — instead of
#                   the bundled demo seed. One site dir, or a parent of many.
#                   Format: docs/architecture/guides/PUBLISH-INGEST-FORMAT.md;
#                   worked example: examples/demo-site/ (make publish INGEST=…).
#   PREFIX=<path>   the per-peer HOSTING SCOPE: nest everything (.html, .bin,
#                   deployment-config origin) under {OUT}/{PREFIX}/… so a domain
#                   can host many isolated peers side by side. Empty (default) =
#                   the domain root, byte-identical to the un-prefixed layout.
#                   Validated (no leading/trailing /, no .., not sites/content).
#   LIVE=<origin>   add the "open in live entity browser" banner ([F2]).
#   HTML_ONLY=1     skip the .bin content data (dumb-CDN-only).
#   DEPLOY_CONFIG=1 also emit /entity-deployment.json (cut 2b) so a GENERIC SPA
#                   bundle served from this origin boots into the published home
#                   site — no per-domain WASM rebuild. SURFACE=<chrome|site|
#                   window> (default window), WINDOW_TYPE=<name> (default
#                   "Site Browser", used when SURFACE=window), LOCKED=1 (kiosk,
#                   SURFACE=site only), CONFIG_SITE=<id> (default demo). Origin =
#                   LIVE if set, else same-origin. chrome = the workspace;
#                   window = maximized window (default a Site Browser), escapable;
#                   site = full-viewport overlay (add LOCKED=1 for a kiosk).
#                   Details:
#                   docs/architecture/guides/GUIDE-DEPLOYMENT-AND-CONFIGURATION.md
#   IDENTITY_SEED=<64-hex>  publish under a SPECIFIC system identity (any
#                   `entity_system_seed`-form hex seed) so each site/deployment
#                   gets its own stable peer-id. Bad seed fails the build.
#                   Default (empty) = the DURABLE publisher identity, load-or-
#                   generated once under PUBLISH_DATA_DIR (see below) and reused
#                   across runs — the peer-id is the site address, so it must not
#                   drift. DEMO_IDENTITY=1 uses the fixed demo seed (dev/testing).
#   PUBLISH_DATA_DIR=<dir>  where the durable publisher keypair lives
#                   (default .entity-publish/, gitignored). Both `publish`
#                   (in-container, via the parent mount) and `publish-serve`
#                   (host) point ENTITY_DATA_DIR here, so both publish under the
#                   SAME identity even though publish runs `podman run --rm`.
# === Apps — the second publish mode: point at a pre-built dist directory ===
# Apps are opaque, self-contained HTML iframe bundles + an `index.json` catalog
# (the entity-apps `dist/` shape). The tool knows nothing app-specific — it just
# ingests a directory via `--ingest-apps=<dir>`. BUILDING that dist is the app
# repo's own concern (exactly as rendering content is the author's), so the
# release pipeline only ever CONSUMES a finished directory — no build step here.
#   APPS_DIST=<dir>   a pre-built apps dist directory to ingest alongside content.
#                     Empty (default) = the bundled demo app seed baked into the
#                     publish binary. Like INGEST/OUT: for the containerized
#                     `publish` target the dir must live UNDER the repo tree (only
#                     the meta dir is bind-mounted); `publish-serve` runs host
#                     cargo and accepts any path. e.g.
#   make publish-serve INGEST=examples/demo-site APPS_DIST=~/path/to/entity-apps/dist
APPS_DIST ?=
# Deployment-config startup surface (used by publish / publish-serve when
# DEPLOY_CONFIG is set): chrome | site | window (default window + a Site Browser
# window type). LOCKED=1 makes a SURFACE=site overlay a kiosk. Ignored without a
# config; the publish binary supplies the defaults when these are empty.
SURFACE ?=
WINDOW_TYPE ?=
LOCKED ?=
OUT ?= dist/static-demo
# Persist the publisher identity across `--rm` runs: point ENTITY_DATA_DIR at the
# repo-local dir, which is visible in-container via the existing parent mount.
publish: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
publish: image
	@mkdir -p $(PUBLISH_DATA_DIR)
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(OUT) $(if $(INGEST),--ingest=$(INGEST),) $(if $(APPS_DIST),--ingest-apps=$(APPS_DIST),) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(LIVE),--live=$(LIVE),) $(if $(HTML_ONLY),--html-only,) $(if $(DEPLOY_CONFIG),--deployment-config,) $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,))

# Bare-root SSG: render ONE site at the domain root (no sites/{peer}/{site}/
# prefix, no entity branding) — the "just a site generator" output. Pick the
# site with `SITE=<id>` (default: the demo site). Output dir = OUT (default
# `dist/static-bare`). Serve with `make serve`-style static server and open /.
OUT_BARE ?= dist/static-bare
publish-bare: image
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(OUT_BARE) --bare-root $(if $(SITE),--site=$(SITE),) $(if $(LIVE),--live=$(LIVE),))

# === THE standard "build everything fresh, publish, and serve" command ===
# One command, ONE origin, to test the whole round-trip end-to-end. It ALWAYS
# rebuilds the SPA first (no stale-bundle guessing — the #1 cause of "the root
# doesn't work / do I have an old build?"), publishes ALL sites (full
# projection, NOT bare) in BOTH forms (legacy-web `.html` + entity-native
# `.bin`) INTO `dist/` itself with the live banner pointed at the SPA, then
# serves `dist/` on a SINGLE port:
#
#   ▶ live entity browser (the SPA)  → http://localhost:PORT/              (default 8081)
#   ▶ static published sites         → http://localhost:PORT/sites/        (same origin)
#
# Same origin: the SPA owns `/`, the static projection owns `/sites/…` (its
# root-absolute links resolve), the `.bin` content data owns `/content/` +
# `/<peer>/`. Browse `/sites/` (lists the sites) → a static page carries the
# "open in live" banner → clicking it lands in the SPA at `/?site=self/…` (the
# static→live round-trip, same origin). The SPA overlay's "Live link" + "Static
# link" share controls round-trip the other way. Publishing into `dist/` writes
# only `sites/`, `content/`, `<peer>/` — it never touches the SPA's index.html
# or bundles.
#   PORT=<n>   the one serve + banner port (default 8081)
#   LIVE=<origin>  ADVANCED. Default EMPTY = same-origin: the published config
#                  and the static→live banner are RELATIVE, so the SAME `dist/`
#                  works at localhost here AND dropped on any CDN/R2 ROOT with no
#                  rebuild (portability is the default — we do NOT bake a domain).
#                  Set LIVE=https://host ONLY to pin a deliberate cross-origin
#                  banner. NEVER set LIVE=http://localhost for a bundle you ship
#                  (it serves a content-less shell off your machine — guarded).
PORT ?= 8081
LIVE ?=
# The serving target (publish-serve) publishes into an ISOLATED
# copy of the SPA bundle here — NOT the shared, git-ignored `dist/`. `make wasm`
# and `make e2e-worker` rebuild/republish `dist/` as a side effect, which would
# otherwise wipe a running deployment out from under you. SERVE_DIR lives under
# /tmp (fully outside the repo), so background work on `dist/` can never touch
# what you're serving. Override SERVE_DIR=<path under /tmp> to relocate.
SERVE_DIR ?= /tmp/entity-serve
# Snapshot the freshly-built `dist/` (the SPA bundle: index.html + wasm + js)
# into SERVE_DIR. SAFETY: the rm is constrained to a path UNDER /tmp via a
# shell case-guard — it can never touch the repo, '/', '$$HOME', or an empty
# value (only a path under /tmp is ever auto-cleaned).
define snapshot_serve_dir
	@dir='$(SERVE_DIR)'; case "$$dir" in \
	  /tmp/?*) ;; \
	  *) echo "refusing SERVE_DIR='$$dir' (only /tmp/* is auto-cleaned; set SERVE_DIR to a path under /tmp)"; exit 1 ;; \
	esac
	@echo "==> snapshot fresh SPA ($(DIST)/) → isolated serve dir $(SERVE_DIR) (background builds can't clobber it)"
	@rm -rf "$(SERVE_DIR)"
	@mkdir -p "$(SERVE_DIR)"
	@cp -a $(DIST)/. "$(SERVE_DIR)/"
endef
# Output-isolated from the canonical dist/ + target/ so this can run
# CONCURRENTLY with `make tauri-run` (which keeps dist/ + target/). Override
# DIST=/TARGET_DIR= to relocate. See the DIST/TARGET_DIR header note.
publish-serve: DIST       := dist-publish
publish-serve: TARGET_DIR := target-publish
# Durable publisher identity, translated to its in-container path (SERVE_DIR is
# mounted below; PUBLISH_DATA_DIR rides the existing parent mount). Mirrors the
# `publish` / `tauri-bundle` targets so all three publish under ONE identity.
publish-serve: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
# publish-serve emits the deployment-config by DEFAULT, so the served SPA
# cold-boots into the published content (registers the publish peer's origin +
# lands on its home site) — otherwise the SPA shows only its own boot seed and
# your published sites appear missing. Turn it off with DEPLOY_CONFIG=0.
publish-serve: DEPLOY_CONFIG := 1
publish-serve: wasm
	$(snapshot_serve_dir)
	@mkdir -p $(PUBLISH_DATA_DIR)
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(SERVE_DIR) $(if $(INGEST),--ingest=$(INGEST),) $(if $(APPS_DIST),--ingest-apps=$(APPS_DIST),) --live=$(LIVE) $(if $(filter-out 0,$(DEPLOY_CONFIG)),--deployment-config,) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,),-v $(SERVE_DIR):$(SERVE_DIR):z)
	@echo ""
	@echo "=== fresh build + published sites — serving on :$(PORT) (one origin, isolated $(SERVE_DIR)) ==="
	@echo "  ▶ live entity browser (SPA):  http://localhost:$(PORT)/"
	@echo "  ▶ static published sites:     http://localhost:$(PORT)/sites/   (banner → live)"
	@echo "  (hard-refresh once if an older build is cached)"
	@echo ""
	$(call RUN_SERVE,$(SERVE_DIR),-v $(SERVE_DIR):$(SERVE_DIR):z)

.PHONY: program-fixtures native test lint wasm wasm-release wasm-test-protocol wasm-measurement e2e-worker e2e-phases e2e-webrtc e2e-webrtc-chat e2e-webrtc-meet e2e-webrtc-nat tauri tauri-run host-run appimage tauri-bundle tauri-bundle-run serve build-serve check-dist publish publish-bare publish-serve

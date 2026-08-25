# Entity Browser — build targets
#
# Active deployments:
#   make wasm         — browser build (DOM), debug
#   make tauri-run    — desktop build (DOM in WebView + native backend peer)
#
# Release paths (see docs/RELEASE-READINESS.md):
#   make dist          — SHIPPABLE installers for this host → artifacts/
#                        (linux: .deb + .rpm + .AppImage · macOS: .dmg ·
#                        windows: .msi + .exe). ADR-0023 Mode 1.
#   make dist-web      — the browser SPA as a release tarball → artifacts/
#   make dist-native   — the same, on the host toolchain (= NATIVE=1). The ONLY
#                        path on macOS/Windows (Tauri's bundler needs the host).
#   make wasm-release  — size-optimized browser SPA → dist/ (unpackaged)
#   make site …        — emit sites/apps content alongside the SPA (CDN deploy)
#   make tauri-bundle  — content-baked desktop app (sites+apps embedded, offline)
#   make appimage      — portable RELEASE .AppImage (also emitted by `dist`)
#   NOTE: `make tauri` / `tauri-run` still produce a DEBUG test binary, not an
#         installer. `dist` is the release path — it builds --release AND
#         packages. Cutting a release is `git tag vN` on green master
#         (ADR-0015); the platform matrix lives in the tag workflow, NOT here.

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
#   make site*     — pure-cargo publish targets (in container)
#
# Host-only targets (NOT part of the bare-box gate, and depend on host services
# or an attached display): the `tools/cors-serve.py` serve steps (serve /
# build-serve / site-serve), `e2e-worker` (external Selenium on :4444), and
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
# containerized `site` target runs `podman run --rm`, so `~/.entity` inside the
# container is ephemeral and the key would regenerate every run — we point
# ENTITY_DATA_DIR at this repo-local dir (which rides the existing parent mount) so
# the identity persists. `site-serve` (host) uses the same dir, so both modes
# publish under the same identity. Override per-machine with PUBLISH_DATA_DIR=…
PUBLISH_DATA_DIR ?= .entity-publish

# Output isolation — so two builds can run AT THE SAME TIME against the same
# source without clobbering each other. `DIST` = trunk's WASM output dir;
# `TARGET_DIR` = cargo's build dir. Both default to the canonical locations,
# so every existing target behaves exactly as before. The `publish-*` family
# overrides them (target-specific vars below) to `dist-publish`/`target-publish`,
# which is what lets `make tauri-run` and `make site-serve` run concurrently.
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

# ============================================================================
# ADR-0019 native opt-in — `NATIVE=1` binds a release recipe to the HOST
# toolchain instead of the container. It is a RUNNER, not a different verb:
# `make dist-native` is exactly `make dist NATIVE=1`, same recipe.
#
# Only the `dist*` family honors it; everything else stays container-only,
# because on Linux the container IS the reproducible answer. The reason the
# switch has to exist at all is macOS and Windows: Tauri's bundler shells out
# to the host's own packaging tools (`hdiutil`/`codesign` for .dmg, WiX/NSIS
# for .msi), so a .dmg can only be built ON macOS and a .msi ON Windows. No
# Linux container can produce them — on those two platforms the native runner
# is the ONLY path, not a convenience.
NATIVE ?=
ifeq ($(NATIVE),1)
DIST_IMAGE_DEP :=
DIST_RUN = CARGO_TARGET_DIR=$(TARGET_DIR) sh -c '$(1)'
else
DIST_IMAGE_DEP := image
DIST_RUN = $(call RUN,$(1))
endif

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
	@echo "    appimage             portable self-contained AppImage (single bundle)"
	@echo "  dist (ADR-0023 Mode 1 — the shippable artifacts):"
	@echo "    dist        installers for THIS host → artifacts/  (linux: deb+rpm+AppImage)"
	@echo "    dist-web    the browser SPA tarball → artifacts/    (host-independent)"
	@echo "    dist-native same recipes on the host toolchain (= NATIVE=1; the only"
	@echo "                path on macOS/Windows). Cutting a release = tag it; CI fans"
	@echo "                out the platform matrix. See docs/RELEASE-READINESS.md."
	@echo "  content: site · site-bare · site-serve · tauri-bundle"
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

# Refuse a publish output directory that lies outside the container's bind mount.
# $(1) = target name (for the message) · $(2) = the path · $(3) = the var's name.
#
# **The failure this exists for is a SUCCESS MESSAGE.** Every publish verb runs
# inside the image, whose only bind mount is $(PARENT). An output path outside it
# resolves to the *container's own* ephemeral filesystem, so the emit prints
# "registry published → …", the chained `--verify` runs in a SECOND container that
# sees an empty directory and reports "this tree is POINTER-TRUSTED … (Not a
# defect)", make exits **0**, and nothing whatsoever is on the host. Measured
# 2026-08-20: `make registry REGISTRY_OUT=/tmp/seqtest2` → exit 0, no such
# directory. `REGISTRY_OUT=/srv/www/registry` — publishing straight into a webroot,
# the obvious thing to try — fails exactly this way, and tells the one person who
# could fix it that it worked.
#
# The rule was already written down (see FED_OUT's comment, and `site-serve`'s
# SERVE_DIR block) and enforced nowhere. `readlink -m` resolves without requiring
# the path to exist, so a sibling repo under $(PARENT) — genuinely inside the
# mount — is still allowed; only a path outside it is refused.
define CHECK_IN_TREE
	@abs=$$(readlink -m '$(2)'); parent=$$(readlink -m '$(PARENT)'); \
	case "$$abs/" in "$$parent"/*) : ;; *) \
	  echo "make $(1): $(3)='$(2)' is outside $$parent."; \
	  echo "  This publish runs in a container that bind-mounts ONLY that directory."; \
	  echo "  An outside path is written to container-ephemeral storage and is gone the"; \
	  echo "  moment the run ends — after printing a success message and exiting 0."; \
	  echo "  Use a repo-relative dir (e.g. $(3)=dist-mysite) and copy it where you want it."; \
	  exit 1 ;; esac
endef

# Serve a static directory ($(1)) from inside the image, on $(PORT). Keeps the
# "podman + make only" contract — no host python3. $(1) is resolved relative to
# the workdir (this repo) OR may be an absolute in-container path (e.g. a mount
# supplied via the $(2) extra-flags param — see site-serve's SERVE_DIR).
# `--network host`: the container binds the host port directly (rootless `-p`
# port-forwarding resets connections under pasta/slirp; host-net is reliable and
# is what a local dev server wants). Foreground; Ctrl-C stops it.
#
# **THROUGH `tools/cors-serve.py`, NEVER `python3 -m http.server` — and the
# reason is a bug this shipped for months.** `http.server` sends no
# `Cache-Control` at all, only `Last-Modified`. A response with no
# `Cache-Control` is not "uncached": browsers apply *heuristic freshness* and
# will serve it from cache without revalidating, so `make wasm` + reload showed
# the OLD build and the only reliable way to see a change was a private window.
# That is the same "opt-IN to immutable" rule `cors-serve.py` already
# implements for a CDN — `no-store` for the mutable shell, `immutable` only for
# bytes whose name is their hash — and every local serve target was bypassing
# the one file that knows it. It also sends the CORS headers, so a local serve
# now behaves like a real deployment in both respects rather than only in the
# one we happened to test. `cors-serve.py` takes (directory, port) positionally
# and binds 0.0.0.0.
define RUN_SERVE
	podman run --rm $(PODMAN_RUN_CAPS) --network host $(2) \
		-v $(PARENT):/src/entity-systems:z \
		-w /src/entity-systems/$(notdir $(CURDIR)) \
		$(IMAGE) \
		python3 tools/cors-serve.py $(1) $(PORT)
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

# One test binary / one test name, in-container — the narrow loop for iterating
# on a single gate without paying for all 14 binaries. Mirrors `make e2e-worker`'s
# `T=` convention. `BIN=` selects a tests/*.rs target, `T=` filters by name;
# both optional. Output is NOT piped, so `$?` is cargo's (see the buildout doc's
# measurement note — a pipeline swallows every stage's status but the last).
#   make test-one BIN=published_root_walk
#   make test-one BIN=published_root_walk T=unwalkable
test-one: image
	$(call RUN,cargo test $(if $(BIN),--test $(BIN),) -- --nocapture --test-threads=1 $(T))

# The desktop backend (src-tauri) is workspace-EXCLUDED (Cargo.toml `exclude`),
# so `make test` does NOT run its unit tests — including the authorize-gate
# manager-grant seeding (manager_grant.rs) and persistence.rs. Run them here,
# in-container (compiles src-tauri, which links webkit2gtk from the image).
test-tauri: image
	$(call RUN,cd src-tauri && cargo test)

# Stand up the whole naming chain locally — N published domains plus one
# registry that names them, as static files, no live peer. Prints the
# name → peer-id mapping and the ONE pin a consumer needs. Edit the DOMAINS
# table at the top of the script to change what gets seeded.
#
# `FED_OUT`, NOT `OUT`: this file already defines `OUT ?= dist/static-demo` for
# the site-bare targets, and reusing it made `make federation` silently write —
# and `rm -rf` — a different target's directory. Per-target output vars get
# per-target names. The path must be INSIDE the repo, since the build runs in a
# container whose only bind mount is the repo.
# **NOT under `dist/`** — audit F10, found by doing it: `make wasm` runs trunk,
# trunk WIPES its dist dir, and the old default `dist/federation` sat inside it.
# So publishing a federation and then building the app silently deleted the
# federation; the next serve 404s and reads as a broken publish. Same hazard the
# `site-serve` block calls out for `SERVE_DIR`, and the same answer: a sibling
# directory. It must stay INSIDE the repo — the container's only bind mount is
# the repo — so a `/tmp` path is not available here the way it is for SERVE_DIR.
FED_OUT ?= dist-federation
federation: EXTRA_RUN_ENV := $(if $(ISSUED_AT_MS),-e ISSUED_AT_MS=$(ISSUED_AT_MS),)
federation: image
	$(call CHECK_IN_TREE,federation,$(FED_OUT),FED_OUT)
	$(call RUN,./tools/local-federation.sh $(FED_OUT))

# ---------------------------------------------------------------------------
# federation-vectors — the COMMITTED cross-impl test-vector corpus.
#
# `dist-federation/` is gitignored, so no commit in any repo contained the
# federation bytes — and workbench-go's fixture is a copy of them, so their
# conformance claim was pinned to "the tree beside which they were cut" rather
# than to the bytes themselves. That is the second time this corpus has paid
# for that (the compute corpus drifted 330 → 343 undetected). Arch offered a
# tag, a committed fixture, or a release artifact; this is the committed
# fixture, which is the only one of the three that pins the bytes *and* stays
# regenerable.
#
# `ISSUED_AT_MS` is fixed here on purpose: `issued_at` rides every binding body,
# so it decides every binding hash and the whole trie shape. Without the pin,
# regenerating produces a different corpus for identical content and the
# fixture cannot be diffed against a fresh emit.
FED_VECTORS := tests/fixtures/registry-federation
FED_VECTORS_ISSUED_AT := 1756000000000
# The corpus README is HAND-WRITTEN and lives inside the corpus directory, which
# the regeneration below wipes — so the documented "regenerate with the same
# command" workflow deleted the one file that explains the corpus, including its
# own Regenerating section. (AGENTS.md says "read that README before
# regenerating"; regenerating removed it.) Carried across rather than moved out
# of the directory, because a fixture that does not carry its own explanation is
# how a consumer ends up cutting from bytes nobody can account for — which is
# the exact failure this corpus was committed to fix.
FED_VECTORS_DOC := /tmp/entity-browser-fed-vectors-README.md
.PHONY: federation-vectors
federation-vectors:
	@test -f $(FED_VECTORS)/README.md && cp $(FED_VECTORS)/README.md $(FED_VECTORS_DOC) || true
	rm -rf $(FED_VECTORS)
	$(MAKE) federation FED_OUT=$(FED_VECTORS) ISSUED_AT_MS=$(FED_VECTORS_ISSUED_AT)
	@test -f $(FED_VECTORS_DOC) && mv $(FED_VECTORS_DOC) $(FED_VECTORS)/README.md || true
	@echo ""
	@echo "  wrote $(FED_VECTORS) — commit it; consumers cut fixtures from these bytes."
	@echo "  Regenerate with the SAME command; a diff means the emitter changed —"
	@echo "  EXCEPT each domain's published-root + its signature (and the two content"
	@echo "  blobs they hash to), which carry a wall clock upstream. Expect 5 modified"
	@echo "  roots and 5 signature/blob renames per run; anything else is the emitter."

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
	rm -rf dist/ dist-publish/ .tauri-home/ .cache/ src-tauri/target/release/bundle/ $(ARTIFACTS)/
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
	$(call RUN,trunk build $(WASM_FEATURES) --dist $(DIST) && ./tools/check-dist.sh $(DIST) && ./tools/build-stamp.sh $(DIST))

# Alias — `make build` is the conventional bare-box entry point across the repo group.
build: wasm

# WASM release build → dist/. Factored into a variable because `dist` and
# `dist-web` run the SAME frontend build through the NATIVE-aware runner —
# one definition, so a release artifact can never be built from a different
# frontend command than `make wasm-release` produces.
WASM_RELEASE_CMD := trunk build --release --dist $(DIST) && ./tools/check-dist.sh $(DIST) && ./tools/build-stamp.sh $(DIST)
wasm-release: image
	$(call RUN,$(WASM_RELEASE_CMD))

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

# The MULTI-HOST federation origin — the publisher on its own host, so a
# consumer's fetches are real network hops rather than loopback ones. Prints the
# two strings a consumer needs (`E2E_FED_ORIGIN`, `E2E_FED_REGISTRY`); the e2e's
# `federation_target()` reads them and refuses a loopback origin.
#
# `federation-multihost` stands the rig up and prints the consumer's environment
# (logs go to stderr, so stdout is pure `KEY=value`); `DOWN=1` tears it down.
# `e2e-federation` is the GATE: rig up → the browser walk against it → rig down,
# with the teardown running whatever the result, so a red run does not leave
# three containers and a network behind.
federation-multihost:
	@bash tools/e2e/federation-multihost.sh $(if $(DOWN),down,up)

# The multi-host gate. Deliberately NOT part of `e2e-worker`: it needs its own
# network and its own browser, and folding it in would make the everyday suite
# depend on both. Run it before claiming the naming chain works off loopback.
e2e-federation: image
	@bash tools/e2e/federation-multihost.sh up > $(FEDENV)
	@cat $(FEDENV)
	# The rig's stdout becomes `-e` flags, so a line that is not KEY=value becomes
	# a bogus env var on the test container. That is not hypothetical: `podman
	# exec -d` printed its exec ID here and shipped `-e <64-hex>` for one run.
	# Cheap check, and it fails the gate instead of quietly mis-configuring it.
	@grep -qvE '^[A-Z0-9_]+=' $(FEDENV) && { echo "FATAL: non-KEY=value line in $(FEDENV):"; cat $(FEDENV); exit 1; } || true
	@set -e; trap 'bash tools/e2e/federation-multihost.sh down >/dev/null 2>&1' EXIT; \
	 $(MAKE) --no-print-directory e2e-federation-run \
	   EXTRA_RUN_ENV="$$(sed 's/^/-e /' $(FEDENV) | tr '\n' ' ')"

FEDENV = target/federation-multihost.env

# The inner half — never call directly; `e2e-federation` supplies the env.
# `--network host` so cargo reaches the browser's published control port; the
# browser's own fetches do not come back this way, they stay on the bridge.
e2e-federation-run:
	$(call RUN,timeout --signal=KILL $(E2E_TIMEOUT) cargo test --features e2e \
	  --test e2e_worker a_name_resolves_cross_origin -- --nocapture --test-threads=1,--network host)

# THE CROSS-IMPL MULTI-HOST LEG — C-7 / `COHORT-OPEN-ITEMS` §1b.
#
# `e2e-federation` closed the TOPOLOGY half: three containers, three distinct
# addresses, a real hop. What it cannot close is the CDN-corridor meta-rule it
# cites, because both ends of that chain are our code — one implementation
# consuming its own emitter across two hosts. This target supplies the other
# implementation: `entity-core-go`'s publisher, stood up through THEIR OWN
# published interface (`scripts/federation-publish.sh`), consumed by our reader.
#
# Two things are deliberate. The test container joins **go's** bridge
# (`--network`), because under rootless podman the host has no route into one —
# so the socket in `crossimpl_go_live.rs` is the consumer's vantage, not an
# orchestrator's. And the contract is renamed `FED_* → GO_FED_*` on the way in,
# so it cannot collide with our own federation rig's variables in a shell that
# has both.
#
# Prerequisite it does NOT hide: go's script builds their binary with the host's
# `go` toolchain. That is one more host tool than this repo's `make + podman`
# floor, and it belongs to their leg, not ours.
GO_REPO   ?= $(PARENT)/entity-core-go
GO_FED_NET ?= entity-go-fed
GOFEDENV   = target/crossimpl-go.env

crossimpl-go: image
	@test -x $(GO_REPO)/scripts/federation-publish.sh || { \
	  echo "FATAL: no $(GO_REPO)/scripts/federation-publish.sh — this leg consumes"; \
	  echo "       entity-core-go's own publisher; it does not reimplement one."; exit 1; }
	@mkdir -p target
	@bash $(GO_REPO)/scripts/federation-publish.sh up > $(GOFEDENV)
	@cat $(GOFEDENV)
	# Same guard as the federation rig, for the same reason: this stdout becomes
	# `-e` flags, and one non-KEY=value line becomes a bogus env var.
	@grep -qvE '^[A-Z0-9_]+=' $(GOFEDENV) && { echo "FATAL: non-KEY=value line in $(GOFEDENV):"; cat $(GOFEDENV); exit 1; } || true
	@set -e; trap 'bash $(GO_REPO)/scripts/federation-publish.sh down >/dev/null 2>&1' EXIT; \
	 bash $(GO_REPO)/scripts/federation-publish.sh probe; \
	 $(MAKE) --no-print-directory crossimpl-go-run \
	   EXTRA_RUN_ENV="$$(sed -e 's/^FED_/GO_FED_/' $(GOFEDENV) | sed -e 's/^/-e /' | tr '\n' ' ')"

# The inner half — never call directly; `crossimpl-go` supplies the env and the
# teardown. Without `GO_FED_ORIGIN` the gates skip loudly rather than passing.
crossimpl-go-run:
	$(call RUN,cargo test crossimpl_go_live -- --include-ignored --nocapture --test-threads=1,--network $(GO_FED_NET))

# List what `T=` and `UNTIL=` accept. Reads the test source, so it can never
# drift from what actually runs — and needs neither the image nor Selenium.
e2e-phases:
	@echo "T=<substring of a test name> — the independent tests:"
	@awk '/^#\[tokio::test/{f=1;next} f&&/^async fn/{sub(/\(.*/,"");print "  " $$3;f=0}' tests/e2e_worker.rs
	@echo
	@echo "UNTIL=<phase> — phases of worker_boots_and_opens_all_windows, in run order:"
	@sed -n '/^const PHASE_ORDER/,/^];/p' tests/e2e_worker.rs | sed -e '1d' -e '$$d' -e 's/^/ /'

# === noscript-check — the apex, seen by something that does not run JS ======
# ============================================================================
# The apex is a WASM SPA, so a crawler that does not execute JavaScript sees an
# EMPTY BODY — while the published sites sit right there under /sites/ as plain
# crawlable HTML that nothing linked to. `index.html` carries a <noscript>
# landing now; this is the only thing that can prove it, because **the e2e
# suite drives a browser with JS ON by definition** and no phase there can ever
# see this surface.
#
# Runs the browser BOTH ways and requires the JS-on run to hide the landing —
# a landing that is always visible would satisfy a one-sided check and would
# mean every ordinary visitor sees the fallback. Mutation-checked in both
# directions (drop the `!important` -> red on visibility; drop the inline
# `display:none` -> red on the control).
#
# NOT in `make lint`: it needs Selenium on :4444, which `lint` must not. Run it
# when index.html changes. **Never beside `e2e-worker`** — that suite's
# `setup()` DELETEs every session on the grid, so the two take each other down.
.PHONY: noscript-check
noscript-check: wasm
	python3 tools/noscript-check.py $(DIST)

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

# SAME LAN, THE WAY A REAL BROWSER DOES IT — row 1 of
# reviews/ANALYSIS-NAT-REACHABILITY. Same shared-bridge topology as
# e2e-webrtc-meet, but with mDNS host-candidate obfuscation ON: candidates carry
# `{uuid}.local` names resolved over multicast, which is what Chrome and Firefox
# actually send and what every other gate here turns OFF so the rig works.
#
# It is a separate target rather than a default because the NAT rigs need raw-IP
# host candidates (`.local` does not resolve between isolated networks, and
# `traverse` is about srflx anyway) — so the pref belongs to the topology that
# is genuinely a LAN, not to the spike.
#
# The gate asserts the pref IS IN EFFECT (at least one `.local` candidate on both
# sides, printed on pass) before crediting the connection. Without that a green
# run cannot be told from the ordinary raw-IP run — Firefox scopes obfuscation by
# permission state, so a set pref that never bites is a real outcome.
#
# Proven 2026-08-19: `.local` on both sides, **0 reflectors**, message delivered
# both ways. Two people on one Wi-Fi need neither STUN nor TURN.
e2e-webrtc-lan:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-lan SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-lan BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-lan: same LAN, mDNS .local host candidates, no reflector"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; E2E_MDNS=1 SPIKE=spike_meet_then_chat.py SPIKE_ARGS="" \
	   bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-lan: PASS — host candidates alone, as a real browser sends them"; \
	 else echo ">>> e2e-webrtc-lan: FAIL (rc=$$rc)"; fi; \
	 exit $$rc

# FILE OVER WEBRTC — the stretch goal's payload. Same ladder as e2e-webrtc-meet
# (nothing handed to the browsers; they add the node, reload, meet at a name),
# but the last phase transfers a FILE instead of a chat message: A `offer`s
# deterministic multi-chunk bytes, B `offers` A to see the manifest, B `pull`s
# and must report the same byte count AND the same content id.
#
# What it proves that the chat gates cannot: a **browser served it**. Every
# other transfer path targets `entity://{peer}/local/files`, whose handler is
# native-only, so two browsers had nobody to receive; this runs over
# `system/content` + an offer manifest, which every peer has. And the payload is
# multi-chunk on purpose — a single-chunk file passes with the receiver's
# closure walk deleted.
#
# Neither browser is ever given the other's address, so there is no WebSocket
# between them to fall back to: bytes that arrive crossed the §6.5 data channel.
FILE_SIZE ?= 700000
e2e-webrtc-file:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-file SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-file BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-file: two browsers meet at a name, then one SERVES a file ($(FILE_SIZE) bytes)"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; FILE_SIZE="$(FILE_SIZE)" SPIKE=spike_file_over_webrtc.py SPIKE_ARGS="" \
	   bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-file: PASS — a file crossed between two browsers"; \
	 else echo ">>> e2e-webrtc-file: FAIL (rc=$$rc) — read which phase failed: offer (serving), offers (the manifest crossing), or pull (the closure walk)"; fi; \
	 exit $$rc

# §4.5.1's AUTOMATIC half: the node is started serving a reflector and PUBLISHES
# it in `advertise`; the browsers are handed nothing and type nothing. They add
# the connector, ask the node what it serves, store the answer on the row, and
# the ICE agent comes up configured.
#
# The contrast with `e2e-webrtc-meet E2E_ICE=…` is the whole point: that proves a
# user CAN type a reflector, this proves they no longer have to. Passing here
# with an empty `ice=` on the connector row is what says the automatic half
# works end to end — node → advertise → registry row → provisioning → ICE agent.
#
# ICE_URI is deliberately a plain (unreachable) STUN address by default: this
# gate asserts the reflector reaches the AGENT, not that it traverses anything.
# `e2e-webrtc-traverse` is what proves traversal, and it drives real media.
ICE_URI ?= stun:198.51.100.7:3478
e2e-webrtc-advertised:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-advertised SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-advertised BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-advertised: the node publishes its reflector (§4.5.1); the browsers type NOTHING"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; E2E_NODE_REFLECTION="$(ICE_URI)" SPIKE=spike_meet_then_chat.py SPIKE_ARGS="" \
	   bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-advertised: PASS"; else echo ">>> e2e-webrtc-advertised: FAIL (rc=$$rc)"; fi; \
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

# The POSITIVE traversal rig: two peers behind two SEPARATE NAT routers, each
# with its own external address, plus a self-hosted STUN responder on the
# transit network (`tools/e2e/webrtc-rung1/nat_topology.sh`). This is the
# topology `e2e-webrtc-nat` cannot build — host masquerade gives both peers ONE
# external address, which is why a STUN result there is unreadable.
#
# GREEN: two peers behind two separate NATs meet at a name and exchange messages
# over WebRTC, first negotiation, 5 offer deposits/side (inside §11.5's bound of
# 8), 3/3 runs. This is the gate that says "works across networks" — the shared
# bridge gates never could.
#
# It was red until the RIG was fixed, not the app: the routers were ACCEPTING
# unsolicited inbound UDP, which confirmed a conntrack entry occupying the exact
# reply tuple each peer's own outbound punch needed, so both sides were remapped
# to ports the other had never heard of. A real NAT drops that packet. See
# nat_topology.sh. The `bucket_full` and `Unknown ufrag` failures that looked
# like app bugs were downstream of it and vanished with it.
#
# Needs `make wasm` first. Builds two small images on first run (a router with
# iptables, a firefox with iproute2); both are cached afterwards.
e2e-webrtc-traverse:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-traverse SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-traverse BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-traverse: two peers, two NATs, one STUN — does media cross?"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; TOPOLOGY=nat SPIKE=spike_meet_then_chat.py SPIKE_ARGS="" \
	   bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-traverse: PASS — media crossed two NATs (the traversal gate)"; \
	 else echo ">>> e2e-webrtc-traverse: FAIL (rc=$$rc) — read the '§6.5 establishment' block: the FIRST failure per side is the informative one, the last reports consequences"; fi; \
	 exit $$rc

# `make e2e-webrtc-idle` — the OTHER half of the §10.3 seam gate.
#
# EXTENSION-NETWORK Amendment 14 requires the connection the seam returns to
# survive idle ("a punched NAT mapping expires on silence and an idle punched
# connection dies in a way no same-host test reproduces"), and §11.5's gate says
# "a direct punched transport that SURVIVES IDLE". `e2e-webrtc-traverse` proves
# establishment and carriage and then tears down — this runs the same rig, goes
# quiet for longer than the routers' UDP mapping lives, and speaks again.
#
# The two knobs are a pair and the gate checks their ratio: IDLE_SECS must
# exceed UDP_TIMEOUT or the mapping was never at risk and the run is refused
# rather than reported green. Defaults 90s quiet against a 30s mapping (3x);
# 30s is inside the range real home routers use for UDP.
#
# It also reports WHICH MECHANISM held the mapping open, by counting packets
# across each router during the quiet window — because if the app chatters, the
# mapping was refreshed by traffic and the run has tested nothing. That number
# is the point, not a diagnostic: arch asked for it so the next substrate can
# reuse the result.
IDLE_SECS ?= 90
UDP_TIMEOUT ?= 30
e2e-webrtc-idle:
	@command -v podman >/dev/null 2>&1 || { echo ">>> e2e-webrtc-idle SKIPPED: podman not found on host"; exit 0; }
ifneq ($(strip $(BUILD)),)
	@$(MAKE) wasm
endif
	@test -f $(DIST)/entity-worker_bg.wasm || { echo "!! $(DIST)/ not built — run 'make wasm' first (or 'make e2e-webrtc-idle BUILD=1')"; exit 1; }
	@echo ">>> e2e-webrtc-idle: two NATs, $(IDLE_SECS)s of silence against a $(UDP_TIMEOUT)s UDP mapping"
	@bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true
	@rc=0; TOPOLOGY=nat SPIKE=spike_meet_then_chat.py SPIKE_ARGS="" \
	   IDLE_SECS=$(IDLE_SECS) UDP_TIMEOUT=$(UDP_TIMEOUT) \
	   bash tools/e2e/webrtc-rung1/rung1_repro.sh || rc=$$?; \
	 bash tools/e2e/webrtc-rung1/rung1_repro.sh teardown >/dev/null 2>&1 || true; \
	 echo; if [ $$rc -eq 0 ]; then echo ">>> e2e-webrtc-idle: PASS — the transport survived $(IDLE_SECS)s of silence (read the 'mechanism' line before quoting this)"; \
	 elif [ $$rc -eq 2 ]; then echo ">>> e2e-webrtc-idle: INCONCLUSIVE — the transport behaved correctly; the RUN could not establish that it was tested. Read the 'mechanism' line: today this exits 2 because the chat poll keeps the link busy at ~50 pkt/s/side, so nothing goes idle. That is a finding, not a failure."; \
	 else echo ">>> e2e-webrtc-idle: FAIL (rc=$$rc) — idle survival genuinely broke (no delivery, or the channel was rebuilt). This is the state that means a regression"; fi; \
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
	@# **WebKitGTK's WebRTC backend is GStreamer, and a host missing its
	@# elements gets a WebView whose RTCPeerConnection constructs and then
	@# gathers nothing.** Not fatal — the app boots, meets, and simply never
	@# connects — which is exactly the shape that cost three sessions to find,
	@# so it warns rather than passing in silence. The container path has these
	@# baked in (see the Dockerfile); host-run borrows the host's.
	@if ! ls /usr/lib64/gstreamer-1.0/libgstwebrtc.so /usr/lib/x86_64-linux-gnu/gstreamer-1.0/libgstwebrtc.so >/dev/null 2>&1; then \
	  echo "!! no GStreamer webrtcbin on this host — peer-to-peer connections will not work."; \
	  echo "   The app will still boot and still MEET other devices; nothing will connect."; \
	  echo "    sudo dnf install gstreamer1-plugins-bad-free gstreamer1-plugins-good   # Fedora"; \
	  echo "    sudo apt install gstreamer1.0-plugins-bad gstreamer1.0-nice            # Debian / Ubuntu"; \
	  echo "   (or run fully containerized: make tauri-run)"; \
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
# THEN embed. Same publish knobs as `make site`
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
# Stage an out-of-mount INGEST / APPS_DIST into repo-local dirs, so a path
# ANYWHERE on the host works with a publish that runs in a container mounting
# only the parent meta dir.
#
# **This is shared by every publish target on purpose.** It lived only on
# `tauri-bundle` for a while, and the consequence was not "site is less
# convenient" — it was that the real sources (papers and entity-apps, which sit
# in a sibling meta tree) had to be hand-copied before `make site` could see
# them at all. A manual copy step is a step you can skip, and skipping the
# APPS_DIST half is exactly the omission that deletes a domain's whole apps
# tree. Convenience here removes a footgun, it does not just save typing.
#
# Always copies when the variable is set, rather than testing whether the path
# happens to be inside the mount: one behaviour is predictable, and "it worked
# on my machine because my checkout was in the right place" is the failure this
# replaces.
#
# Skipped entirely under VERIFY=1: `--verify` walks the OUTPUT directory and
# never reads a source (that is the whole point — a shipped tree can be checked
# without still having what produced it), so staging there would copy a
# multi-hundred-megabyte corpus to look at none of it.
define stage_publish_sources
	$(if $(VERIFY),,@rm -rf $(INGEST_STAGE) $(APPS_STAGE))
	$(if $(VERIFY),,$(if $(INGEST),@echo "==> staging INGEST=$(INGEST) → $(INGEST_STAGE)/ (the publish container mounts only this repo)"))
	$(if $(VERIFY),,$(if $(INGEST),@cp -r $(INGEST) $(INGEST_STAGE)))
	$(if $(VERIFY),,$(if $(APPS_DIST),@echo "==> staging APPS_DIST=$(APPS_DIST) → $(APPS_STAGE)/"))
	$(if $(VERIFY),,$(if $(APPS_DIST),@cp -r $(APPS_DIST) $(APPS_STAGE)))
endef
define unstage_publish_sources
	@rm -rf $(INGEST_STAGE) $(APPS_STAGE)
endef
# The flags every staged publish passes — the staged paths, never the caller's.
INGEST_STAGED_FLAG = $(if $(INGEST),--ingest=$(INGEST_STAGE),)
APPS_STAGED_FLAG   = $(if $(APPS_DIST),--ingest-apps=$(APPS_STAGE),)
tauri-bundle: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
tauri-bundle: wasm-release
	@mkdir -p $(PUBLISH_DATA_DIR)
	$(stage_publish_sources)
	$(call RUN,cargo run --quiet --bin entity-browser -- publish dist $(INGEST_STAGED_FLAG) $(APPS_STAGED_FLAG) $(if $(LIVE),--live=$(LIVE),) --deployment-config $(if $(REGISTRY_PIN),--registry-pin=$(REGISTRY_PIN),) $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,))
	$(unstage_publish_sources)
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

# ---------------------------------------------------------------------------
# pair-serve — serve an app that is ALREADY paired with this machine's node.
#
# THE PAIRING PROBLEM, AND WHY THIS IS THE ANSWER. Joining a rendezvous means
# getting a Base58 peer id onto the other device. Every channel for that is
# bad: retyping it across a room is where a real two-machine run actually
# stalls (and a typo presents as a connectivity failure), and the QR scanner
# needs a camera, which `getUserMedia` denies on the plain-http LAN origin you
# are necessarily on.
#
# But the side channel was never needed: **this machine is already serving the
# app to that device**, so the URL they have to type anyway can BE the pairing.
# `build.rs` bakes the node into the bundle; a browser that merely LOADS
# `http://<this-ip>:$(PORT)` is provisioned — nothing to copy, no QR, no
# reload, no Shell verb.
#
# The identity store makes discovery free: a backend peer is stored as
# `~/.entity/backend-peers/{peer_id}` — **the filename IS the peer id** — so
# this needs no crypto and no running Tori.
#
# Override either half explicitly:
#   make pair-serve NODE_PEER=2K… NODE_ADDR=ws://10.0.0.5:4041
BACKEND_PEER_DIR := $(HOME)/.entity/backend-peers
# Newest FIRST (`ls -t`). The sidecar `.meta` files are empty in this layout —
# no label, no flags, nothing to choose by — so mtime is the only real signal
# on disk, and it is the right one: the identity Tori touched most recently is
# the one it is running. Defaulting to it and SAYING SO beats asking the
# operator to pick between two opaque Base58 strings using information that does
# not exist (which is what the first version of this did).
NODE_CANDIDATES := $(shell ls -1t $(BACKEND_PEER_DIR) 2>/dev/null | grep -vE '\.(meta|pub|toml)$$')
NODE_PEER ?= $(firstword $(NODE_CANDIDATES))
LAN_IP ?= $(shell ip -4 addr show scope global 2>/dev/null | grep -oE 'inet [0-9.]+' | awk '{print $$2}' | head -1)
NODE_ADDR ?= $(if $(LAN_IP),ws://$(LAN_IP):4041,)

pair-serve: EXTRA_RUN_ENV := -e ENTITY_WEBRTC_NODE_PEER=$(NODE_PEER) -e ENTITY_WEBRTC_NODE_ADDR=$(NODE_ADDR)
pair-serve: pair-check wasm-release
	@echo ""
	@echo "=== serving a PRE-PAIRED build ==="
	@echo "  node    $(NODE_PEER)"
	@echo "  at      $(NODE_ADDR)"
	@echo ""
	@echo "  On the phone / the other computer, open:"
	@echo "      http://$(LAN_IP):$(PORT)"
	@echo "  That is the whole pairing. Nothing to copy, nothing to type."
	@echo "  Then: Peer Connections -> Meet -> same word on both -> File Transfer."
	@echo ""
	$(call RUN_SERVE,dist)

# Refuse BEFORE the release build rather than after it. A missing half bakes
# nothing (build.rs panics on a half-config), and the ambiguous case must be a
# question rather than a guess: picking one of several identities silently
# would serve a build pointed at a node nobody is running, which fails as "the
# rendezvous is empty" — the failure this target exists to abolish.
.PHONY: pair-check
pair-check:
	@if [ -z "$(NODE_PEER)" ]; then \
	  echo "pair-serve: no backend peer identity found in $(BACKEND_PEER_DIR)."; \
	  echo "  Start Tori once so it creates one:"; \
	  echo "    make host-run   then  System Overview -> Rendezvous -> Start"; \
	  exit 1; \
	fi
	@echo "using node identity  $(NODE_PEER)"
	@echo "  (most recently used in $(BACKEND_PEER_DIR))"
	@if [ "$(words $(NODE_CANDIDATES))" -gt 1 ]; then \
	  echo "  other identities on this machine, newest first:"; \
	  for p in $(filter-out $(NODE_PEER),$(NODE_CANDIDATES)); do \
	    echo "    $$p   last used $$(date -r $(BACKEND_PEER_DIR)/$$p '+%Y-%m-%d %H:%M')"; \
	  done; \
	  echo "  override with: make pair-serve NODE_PEER=<id>"; \
	fi
	@echo "  Tori shows the id it is actually running in System Overview -> Rendezvous."
	@echo "  If it differs from the line above, pass it explicitly."
	@if [ -z "$(NODE_ADDR)" ]; then \
	  echo "pair-serve: could not determine this machine's LAN address."; \
	  echo "  Pass it: make pair-serve NODE_ADDR=ws://<this-machine-ip>:4041"; \
	  exit 1; \
	fi

# Publish — render the site set to static no-JS HTML (the legacy-web /
# CDN / permalink projection). Headless native, no browser: builds a peer,
# reads its sites off the tree ([A] reader), projects them to
# `dist/static-demo/sites/{peer}/{site}/…` ([B1] emitter). Override the
# output dir: `make site OUT=path/to/dir`. OUT must stay UNDER the repo tree
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
#                   worked example: examples/demo-site/ (make site INGEST=…).
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
#   REGISTRY_PIN=<PEER_ID[@ORIGIN]>  seed the §7.4 preloaded NAME REGISTRY into
#                   /entity-deployment.json (`name_registry_pin`), so a visitor
#                   resolves names through that registry before typing anything.
#                   Same PEER_ID@ORIGIN spelling as `registry --bind` — it is the
#                   same pair, and a second spelling is a second thing to get
#                   wrong. Bare peer-id = same origin. **Needs DEPLOY_CONFIG=1**:
#                   the pin rides in that file, and `publish` refuses the
#                   combination without it rather than emitting a pin nothing
#                   carries.
#                   Validated at the emitter (`parse_registry_pin`) — a
#                   non-canonical peer-id is refused HERE, where an operator can
#                   read the refusal, rather than dropped silently at a
#                   consumer's machine (audit F9's rule). That validation is the
#                   entire reason this flag exists as a passthrough: without it
#                   the only route was hand-editing the emitted JSON, which
#                   bypasses exactly the check the emitter is for. Honoured by
#                   `site`, `site-dist` (inherited), `site-serve` and
#                   `tauri-bundle`; NOT by `site-bare`, which emits no
#                   deployment config at all.
#   IDENTITY_SEED=<64-hex>  publish under a SPECIFIC system identity (any
#                   `entity_system_seed`-form hex seed) so each site/deployment
#                   gets its own stable peer-id. Bad seed fails the build.
#                   Default (empty) = the DURABLE publisher identity, load-or-
#                   generated once under PUBLISH_DATA_DIR (see below) and reused
#                   across runs — the peer-id is the site address, so it must not
#                   drift. DEMO_IDENTITY=1 uses the fixed demo seed (dev/testing).
#   PUBLISH_DATA_DIR=<dir>  where the durable publisher keypair lives
#                   (default .entity-publish/, gitignored). Both `site`
#                   (in-container, via the parent mount) and `site-serve`
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
#                     `site` target the dir must live UNDER the repo tree (only
#                     the meta dir is bind-mounted); `site-serve` runs host
#                     cargo and accepts any path. e.g.
#   make site-serve INGEST=examples/demo-site APPS_DIST=~/path/to/entity-apps/dist
APPS_DIST ?=
# Deployment-config startup surface (used by publish / site-serve when
# DEPLOY_CONFIG is set): chrome | site | window (default window + a Site Browser
# window type). LOCKED=1 makes a SURFACE=site overlay a kiosk. Ignored without a
# config; the publish binary supplies the defaults when these are empty.
SURFACE ?=
WINDOW_TYPE ?=
LOCKED ?=
OUT ?= dist/static-demo
# PLAN=1 — resolve the sources and report what the publish WOULD add/keep/remove,
# writing nothing. A publish replaces the whole projection under its prefix, so a
# subset of the source set DELETES the rest; this is how you find that out first.
#   exit 0 = nothing removed · exit 2 = the plan REMOVES sites · exit 1 = error
# So `make site PLAN=1 … && make site …` is a gate that refuses to run a
# destructive publish, with no extra flags. Same knobs as a real publish — plan
# what you are about to run, not an approximation of it.
PLAN ?=
# The §7.4 preloaded registry pin (see the knob table above). Empty = no pin,
# which is the shipped default: a catch-all registry chosen for everybody is how
# two app tiers ship two, so a pin is always a per-deployment decision.
REGISTRY_PIN ?=
# VERIFY=1 — walk an ALREADY-published OUT dir and prove the two-hop chain
# resolves: every .bin pointer cracks, names a blob that exists, and whose bytes
# hash to the address it claims. Runs the SAME check the browser runs per fetch,
# so a clean result means a visitor's client will not reject anything either.
# Needs no sources (it reads the output dir), so it works on a tree you kept.
#   exit 0 = clean · exit 2 = BROKEN pointers · exit 1 = error
# Orphan blobs are reported, not failed — nothing links to them, and a tree
# mid-cutover legitimately holds blobs its pointers have not adopted yet.
VERIFY ?=
# Persist the publisher identity across `--rm` runs: point ENTITY_DATA_DIR at the
# repo-local dir, which is visible in-container via the existing parent mount.
site: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
site: image
	$(call CHECK_IN_TREE,site,$(OUT),OUT)
	@mkdir -p $(PUBLISH_DATA_DIR)
	$(stage_publish_sources)
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(OUT) $(if $(PLAN),--plan,) $(if $(VERIFY),--verify,) $(INGEST_STAGED_FLAG) $(APPS_STAGED_FLAG) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(LIVE),--live=$(LIVE),) $(if $(HTML_ONLY),--html-only,) $(if $(DEPLOY_CONFIG),--deployment-config,) $(if $(REGISTRY_PIN),--registry-pin=$(REGISTRY_PIN),) $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,))
	$(unstage_publish_sources)

# ============================================================================
# === site-dist — THE UPLOADABLE PRODUCTION WEB TREE =========================
# ============================================================================
# **This is the artifact a domain actually serves, and until now it had no
# name.** `make site` emits the CONTENT HALF ONLY: its root index.html is a
# redirect to /sites/ and there is no wasm. Uploading that to a bucket root
# REPLACES THE LIVE SPA WITH A REDIRECT PAGE and orphans every app bundle —
# `.bin` bundles under {peer}/apps/** reachable only through the SPA, which
# nothing in the static HTML projection links to. Production serves the SPA at
# the apex with content under /sites/ (verified: entitycoreprotocol.org returns
# <title>Entity Browser</title> and loads entity-browser-*_bg.wasm).
#
# The composition existed in two places and neither emitted an uploadable tree:
# `site-serve` builds it and SERVES it, `tauri-bundle` builds it and EMBEDS it
# in a desktop binary. So the only written record of the rule was a comment on
# a DESKTOP target, which is not where anyone doing a web release is reading.
#
# THE ORDERING IS LOAD-BEARING: wasm-release FIRST (trunk WIPES its dist dir),
# THEN publish INTO it (the publish cleans only its own roots — sites/, content/,
# {peer}/ — leaving the SPA). Reverse them and you ship a content-less shell.
#
# It is TWO SUB-MAKES rather than two prerequisites, deliberately: prerequisites
# are order-independent under `make -j`, and the one thing this target exists to
# guarantee is an order. A parallel build would race trunk's wipe against the
# publish and produce a tree that is wrong intermittently — the worst possible
# failure for a release artifact. Command-line knobs (INGEST, APPS_DIST,
# DEMO_IDENTITY, PREFIX, LIVE, CONFIG_SITE, …) propagate to sub-makes on their
# own; INGEST/APPS_DIST may point ANYWHERE (they are staged, see
# stage_publish_sources).
#
# DEPLOY_CONFIG defaults to 1 here for the same reason site-serve defaults it:
# without /entity-deployment.json the SPA boots to its own seed and every
# published site appears MISSING. A production upload needs it, and nothing
# else enforces that.
#
#   make site-dist INGEST=<papers/render/output> APPS_DIST=<entity-apps/dist>
#   make site-dist SITE_DIST_OUT=dist-ecp CONFIG_SITE=home LIVE=https://example.org
SITE_DIST_OUT ?= dist-site
SITE_DIST_TARGET ?= target-publish
SITE_DIST_DEPLOY_CONFIG ?= 1
site-dist:
	$(MAKE) wasm-release DIST=$(SITE_DIST_OUT) TARGET_DIR=$(SITE_DIST_TARGET)
	$(MAKE) site OUT=$(SITE_DIST_OUT) DEPLOY_CONFIG=$(SITE_DIST_DEPLOY_CONFIG)
	@echo ""
	@echo "==> verifying the tree we are about to be able to upload"
	$(MAKE) site OUT=$(SITE_DIST_OUT) VERIFY=1
	@echo ""
	@echo "=== uploadable web tree: $(SITE_DIST_OUT)/ ==="
	@echo "  objects: $$(find $(SITE_DIST_OUT) -type f | wc -l)   size: $$(du -sh $(SITE_DIST_OUT) | cut -f1)"
	@echo "  apex:    SPA (index.html + wasm)   content: sites/ + content/"
	@echo "  apps:    $$(find $(SITE_DIST_OUT) -path '*/apps/*/bundles/*.bin' | wc -l) bundle(s)"
	@echo "  config:  $$(test -f $(SITE_DIST_OUT)/entity-deployment.json && echo entity-deployment.json || echo 'MISSING — the SPA will show no published sites')"
	@echo "  serve it locally:  make serve DIST=$(SITE_DIST_OUT)"

# Bare-root SSG: render ONE site at the domain root (no sites/{peer}/{site}/
# prefix, no entity branding) — the "just a site generator" output. Pick the
# site with `SITE=<id>` (default: the demo site). Output dir = OUT (default
# `dist/static-bare`). Serve with `make serve`-style static server and open /.
OUT_BARE ?= dist/static-bare
site-bare: image
	$(call CHECK_IN_TREE,site-bare,$(OUT_BARE),OUT_BARE)
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(OUT_BARE) --bare-root $(if $(SITE),--site=$(SITE),) $(if $(LIVE),--live=$(LIVE),))

# Publish a NAME REGISTRY — the second half of the naming chain, and until now
# the only publish verb with no make target (audit F3): the sole route was a bare
# `cargo run`, which a podman-only host does not have. `make site` publishes
# content; this publishes the names that point at it, under a DIFFERENT identity
# (a name-issuer that is also the thing it names is one key doing two jobs).
#
#   make registry BIND='--bind=foundation.example=2KGT…@https://foundation.example'
#   make registry REGISTRY_OUT=dist-reg TTL_DAYS=90 BIND='--bind=a=PID@ORIGIN --bind=b=PID2@ORIGIN2'
#
# Keep REGISTRY_OUT OUT of `dist/` — trunk wipes it (see the `federation` note).
#
# BIND is passed through verbatim, so every flag keeps its `=` (the CLI takes no
# space-separated values — see `registry_publish::run`). The @ORIGIN half is
# REQUIRED (arch D10). The identity is the durable registry keypair under
# $(PUBLISH_DATA_DIR) unless you pass --identity-seed= inside SEED.
# `make federation` is the batteries-included version of this for local testing.
REGISTRY_OUT ?= dist-registry
registry: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
registry: image
	@[ -n "$(BIND)" ] || { echo "make registry: BIND is required, e.g."; \
	  echo "  make registry BIND='--bind=NAME=PEER_ID@ORIGIN'"; \
	  echo "  (the @ORIGIN half is not optional — arch D10)"; exit 1; }
	$(call CHECK_IN_TREE,registry,$(REGISTRY_OUT),REGISTRY_OUT)
	@mkdir -p $(PUBLISH_DATA_DIR)
	$(call RUN,cargo run --quiet --bin entity-browser -- registry $(REGISTRY_OUT) $(BIND) $(if $(TTL_DAYS),--ttl-days=$(TTL_DAYS),) $(SEED))
	@echo ""
	@echo "==> verifying the tree we just emitted (the chain hangs from this root)"
	$(call RUN,cargo run --quiet --bin entity-browser -- registry $(REGISTRY_OUT) $(SEED) --verify)

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
# The serving target (site-serve) publishes into an ISOLATED
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
site-serve: DIST       := dist-publish
site-serve: TARGET_DIR := target-publish
# Durable publisher identity, translated to its in-container path (SERVE_DIR is
# mounted below; PUBLISH_DATA_DIR rides the existing parent mount). Mirrors the
# `site` / `tauri-bundle` targets so all three publish under ONE identity.
site-serve: EXTRA_RUN_ENV := -e ENTITY_DATA_DIR=/src/entity-systems/$(notdir $(CURDIR))/$(PUBLISH_DATA_DIR)
# site-serve emits the deployment-config by DEFAULT, so the served SPA
# cold-boots into the published content (registers the publish peer's origin +
# lands on its home site) — otherwise the SPA shows only its own boot seed and
# your published sites appear missing. Turn it off with DEPLOY_CONFIG=0.
site-serve: DEPLOY_CONFIG := 1
site-serve: wasm
	$(snapshot_serve_dir)
	@mkdir -p $(PUBLISH_DATA_DIR)
	$(stage_publish_sources)
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(SERVE_DIR) $(INGEST_STAGED_FLAG) $(APPS_STAGED_FLAG) --live=$(LIVE) $(if $(filter-out 0,$(DEPLOY_CONFIG)),--deployment-config,) $(if $(REGISTRY_PIN),--registry-pin=$(REGISTRY_PIN),) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,),-v $(SERVE_DIR):$(SERVE_DIR):z)
	$(unstage_publish_sources)
	@echo ""
	@echo "=== fresh build + published sites — serving on :$(PORT) (one origin, isolated $(SERVE_DIR)) ==="
	@echo "  ▶ live entity browser (SPA):  http://localhost:$(PORT)/"
	@echo "  ▶ static published sites:     http://localhost:$(PORT)/sites/   (banner → live)"
	@echo "  (hard-refresh once if an older build is cached)"
	@echo ""
	$(call RUN_SERVE,$(SERVE_DIR),-v $(SERVE_DIR):$(SERVE_DIR):z)

# ============================================================================
# === dist — ADR-0023 Mode 1: the shippable artifact for THIS host ===========
# ============================================================================
# `make dist` produces the end-user installers for the host you are standing
# on. `make dist-web` produces the browser SPA tarball (host-independent).
# Both land in $(ARTIFACTS)/ under the fleet-wide filename scheme
# `{name}_{version}_{os}_{arch}.{ext}` (ADR-0023).
#
# WHERE THE PLATFORM MATRIX LIVES — read this before adding a target here.
# It is NOT in this file. ADR-0023 gives the matrix exactly ONE home so it
# cannot drift, and for this repo that home is the tag-triggered workflow's
# `strategy.matrix` (.github/workflows/release.yml). There is deliberately no
# `dist-linux` / `dist-windows` / `dist-macos` family (ADR-0023 rejects it as
# the industry least-favored, drift-prone shape) and no `make release`
# (a release is an ACT — `git tag vN` on green master, ADR-0015). Adding a
# platform is a one-line add to that matrix. `make` only ever knows the host.
#
# There is also no `dist-all`: ADR-0023 permits one as a local dry-run escape
# hatch, but it assumes GoReleaser's cross-compiler. We bundle with Tauri
# (docs/adr/0002 — a .dmg needs macOS, a .msi needs Windows), so a `dist-all`
# on a Linux box could only ever produce the Linux third of a release while
# its name promised the whole thing. A verb that lies is worse than no verb.
DIST_NAME    ?= entity-browser
# Single source of truth for the version: the crate. src-tauri/Cargo.toml and
# tauri.conf.json carry the same string — `make dist` FAILS if they disagree
# (see the version-coherence guard below), so a release can't ship a binary
# whose installer metadata claims a different version.
DIST_VERSION ?= $(shell sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)
ARTIFACTS    ?= artifacts

# Host detection. `_host_*` is what this machine IS and is never overridden;
# `DIST_*` is what you are asking for and defaults to the host. Overriding
# DIST_OS is how you cross-build: `make dist DIST_OS=windows`.
_uname_s := $(shell uname -s)
_uname_m := $(shell uname -m)
ifeq ($(_uname_s),Linux)
  _host_os := linux
endif
ifeq ($(_uname_s),Darwin)
  _host_os := macos
endif
ifneq (,$(findstring MINGW,$(_uname_s))$(findstring MSYS,$(_uname_s))$(findstring CYGWIN,$(_uname_s)))
  _host_os := windows
endif
_host_os ?= unknown
ifneq (,$(filter x86_64 amd64,$(_uname_m)))
  _host_arch := x64
endif
ifneq (,$(filter aarch64 arm64,$(_uname_m)))
  _host_arch := arm64
endif
_host_arch ?= $(_uname_m)

DIST_OS   ?= $(_host_os)
DIST_ARCH ?= $(_host_arch)

# Which bundle formats, keyed on the REQUESTED os — not the host, or asking for
# windows on Linux would try to build .deb.
ifeq ($(DIST_OS),linux)
  # Tauri bundles all three natively on Linux — no host dpkg/rpmbuild needed.
  DIST_BUNDLES ?= deb,rpm,appimage
endif
ifeq ($(DIST_OS),macos)
  DIST_BUNDLES ?= dmg,app
endif
ifeq ($(DIST_OS),windows)
  ifeq ($(_host_os),windows)
    DIST_BUNDLES ?= msi,nsis
  else
    # Cross-building: NSIS only. `.msi` is WiX, and tauri-bundler gates
    # `PackageType::WindowsMsi` behind `#[cfg(target_os = "windows")]` — off a
    # Windows host that arm does not exist and the request is silently
    # *ignored* ("ignoring msi"), so asking for it would hand you a release
    # that is quietly missing a file. NSIS is deliberately NOT gated; the
    # bundler's own comment says "don't restrict to windows as NSIS installers
    # can be built in linux+macOS using cargo-xwin".
    DIST_BUNDLES ?= nsis
  endif
endif
DIST_BUNDLES ?=

# --- Cross-building ---------------------------------------------------------
# Empty when building for the host, which is the normal case.
#
# Windows-from-Linux WORKS and is proven here: cargo-xwin supplies the MSVC
# CRT/SDK, clang+lld link the PE, and Tauri drives makensis. What you get is
# `_setup.exe`, not `.msi` (see above).
#
# macOS is NOT cross-buildable and this is not a licence quibble: in
# tauri-bundler `mod macos` is itself `#[cfg(target_os = "macos")]`, so the
# `.app`/`.dmg` bundlers are compiled OUT of the binary on any other host —
# there is no code to call. (Underneath that, linking Cocoa/WebKit needs
# Apple's SDK, whose licence restricts use to Apple hardware.) macOS artifacts
# come from a macOS runner; the release workflow uses GitHub's free ones.
DIST_TARGET :=
DIST_CROSS  :=
_cross_unsupported :=
ifneq ($(DIST_OS),$(_host_os))
  ifeq ($(DIST_OS),windows)
    ifeq ($(DIST_ARCH),arm64)
      DIST_TARGET := aarch64-pc-windows-msvc
    else
      DIST_TARGET := x86_64-pc-windows-msvc
    endif
    DIST_CROSS := --runner cargo-xwin --target $(DIST_TARGET)
  else
    _cross_unsupported := 1
  endif
endif

# cargo puts a cross build under target/<triple>/, a host build directly under
# target/ — the staging step has to be told which.
DIST_BUNDLE_ROOT := src-tauri/$(TARGET_DIR)$(if $(DIST_TARGET),/$(DIST_TARGET),)/release/bundle

# cargo-xwin downloads Microsoft's CRT and Windows SDK headers, which is a
# licence you accept, not one we can accept on your behalf in a committed file.
# One env var, once.
XWIN_ACCEPT_LICENSE ?=
define check_cross_supported
	@if [ -n "$(_cross_unsupported)" ]; then \
	  echo "make dist: cannot build $(DIST_OS) artifacts on a $(_host_os) host."; \
	  echo "  tauri-bundler compiles its macOS bundlers out entirely off a macOS host"; \
	  echo "  (\`mod macos\` is #[cfg(target_os = \"macos\")]) — there is nothing to call."; \
	  echo "  Use a macOS machine or the macos leg of .github/workflows/release.yml."; \
	  exit 1; \
	fi
	@if [ -n "$(DIST_CROSS)" ] && [ -z "$(XWIN_ACCEPT_LICENSE)" ]; then \
	  echo "Cross-building Windows needs Microsoft's CRT + Windows SDK headers,"; \
	  echo "which cargo-xwin downloads under Microsoft's licence. Accept it explicitly:"; \
	  echo ""; \
	  echo "    make dist DIST_OS=windows XWIN_ACCEPT_LICENSE=1"; \
	  echo ""; \
	  exit 1; \
	fi
endef

# Guard: the version string lives in three files that a release must agree on.
# Cheap to check, and the failure it prevents (an installer that advertises
# 0.8.0 while the binary is 0.9.0) is invisible until a user reports it.
define check_dist_version
	@v='$(DIST_VERSION)'; [ -n "$$v" ] || { echo "dist: could not read version from Cargo.toml"; exit 1; }; \
	t=$$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' src-tauri/Cargo.toml | head -1); \
	j=$$(sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1); \
	[ "$$v" = "$$t" ] && [ "$$v" = "$$j" ] || { \
	  echo "dist: version mismatch — Cargo.toml=$$v src-tauri/Cargo.toml=$$t tauri.conf.json=$$j"; \
	  echo "      all three must match before a release can be built."; exit 1; }
endef

# Preflight for the native runner: in the container these are guaranteed by
# the image, on a host they are the contributor's to install, and the error
# cargo gives ("no such subcommand: tauri") does not say how to fix it.
define check_native_toolchain
	@if [ "$(NATIVE)" = "1" ]; then \
	  command -v cargo >/dev/null 2>&1 || { echo "dist-native needs a host Rust toolchain (rustup.rs); or drop NATIVE=1 to build in the container."; exit 1; }; \
	  cargo tauri --version >/dev/null 2>&1 || { echo "dist-native needs the Tauri CLI:  cargo install --locked tauri-cli --version '^2'"; exit 1; }; \
	  command -v trunk >/dev/null 2>&1 || { echo "dist-native needs Trunk:  cargo install --locked trunk@0.21.14  (+ rustup target add wasm32-unknown-unknown)"; exit 1; }; \
	fi
endef

# The desktop installers for this host. Ships the GENERIC app (no baked site
# content) — that is the thing an end user installs. A content-baked
# distributable is a different product: publish into dist/ first, exactly as
# `tauri-bundle` does, then bundle.
#
# NO_STRIP=1 + APPIMAGE_EXTRACT_AND_RUN=1 are inherited from the proven
# `appimage` target: the container has no FUSE, so linuxdeploy/appimagetool
# must self-extract, and its strip pass is unreliable there.
# Preflight is its OWN prerequisite, ahead of `image`, so an impossible request
# (macOS from Linux) is refused in a second instead of after a container build.
dist-preflight:
	@[ -n "$(DIST_BUNDLES)" ] || { echo "make dist: unrecognized target '$(DIST_OS)/$(DIST_ARCH)' (host $(_uname_s)/$(_uname_m))."; \
	  echo "  Set them explicitly:  make dist DIST_OS=linux DIST_ARCH=x64 DIST_BUNDLES=deb,rpm,appimage"; exit 1; }
	$(check_cross_supported)

dist: dist-preflight $(DIST_IMAGE_DEP)
	$(check_dist_version)
	$(check_native_toolchain)
	@echo "==> dist $(DIST_NAME) $(DIST_VERSION) — $(DIST_OS)/$(DIST_ARCH) [$(DIST_BUNDLES)]$(if $(DIST_TARGET), cross → $(DIST_TARGET),)$(if $(NATIVE), (native runner),)"
	$(call DIST_RUN,$(WASM_RELEASE_CMD))
	$(call DIST_RUN,touch src-tauri/src/lib.rs && cd src-tauri && APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=1 XWIN_ACCEPT_LICENSE=$(XWIN_ACCEPT_LICENSE) cargo tauri build $(DIST_CROSS) --bundles $(DIST_BUNDLES))
	@./tools/dist-stage.sh "$(DIST_NAME)" "$(DIST_VERSION)" "$(DIST_OS)" "$(DIST_ARCH)" "$(DIST_BUNDLE_ROOT)" "$(ARTIFACTS)"

# The browser SPA as a release artifact — the static bundle you drop on a CDN
# or any static origin. Host-independent (it is wasm), so CI builds it ONCE
# rather than per matrix leg, and its filename carries no os/arch.
dist-web: $(DIST_IMAGE_DEP)
	$(check_dist_version)
	$(check_native_toolchain)
	@mkdir -p $(ARTIFACTS)
	$(call DIST_RUN,$(WASM_RELEASE_CMD))
	$(call DIST_RUN,tar -czf $(ARTIFACTS)/$(DIST_NAME)_$(DIST_VERSION)_web.tar.gz -C $(DIST) .)
	@echo ""
	@echo "=== web SPA artifact ==="
	@ls -lh $(ARTIFACTS)/$(DIST_NAME)_$(DIST_VERSION)_web.tar.gz | awk '{print "  " $$9 "  (" $$5 ")"}'

# ADR-0019's `-native` opt-in spelled as a verb. Same recipes, host toolchain.
dist-native:
	@$(MAKE) dist NATIVE=1
dist-web-native:
	@$(MAKE) dist-web NATIVE=1

# --- ADR-0023 Amendment 1: `publish` is a RESERVED verb ---------------------
# It means ONE thing fleet-wide — push a built package to its language's
# native registry (crates.io/npm/…). This repo has no such package, so it does
# not define the verb; what used to live here (emit a content site to a
# directory) is now `site` / `site-bare` / `site-serve`.
#
# These stubs exist ONLY to convert stale muscle memory and stale scripts into
# a readable message instead of "No rule to make target". Delete them after
# one release. NOTE the app's own CLI is a separate namespace and is
# UNCHANGED: `entity-browser publish <dir>` is still the command, and
# PUBLISH_DATA_DIR/PUBLISH-INGEST-FORMAT still name it — the reservation
# governs `make` verbs only.
publish publish-bare publish-serve:
	@echo 'make $@ was renamed — "publish" is a reserved verb (ADR-0023 Amendment 1).'
	@echo ''
	@echo '    make $(patsubst publish%,site%,$@)'
	@echo ''
	@echo '  Same recipe, same knobs, new name. The app CLI is a different'
	@echo '  namespace and did NOT change: entity-browser publish <dir>'
	@exit 1

.PHONY: e2e-webrtc-file program-fixtures native test lint wasm wasm-release wasm-test-protocol wasm-measurement e2e-worker e2e-phases e2e-webrtc e2e-webrtc-chat e2e-webrtc-meet e2e-webrtc-nat tauri tauri-run host-run appimage tauri-bundle tauri-bundle-run serve build-serve check-dist site site-dist site-bare site-serve dist dist-preflight dist-web dist-native dist-web-native publish publish-bare publish-serve e2e-webrtc-advertised e2e-webrtc-traverse e2e-webrtc-idle e2e-webrtc-lan


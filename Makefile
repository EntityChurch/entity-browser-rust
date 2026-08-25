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

.PHONY: image build help fmt check clean

.DEFAULT_GOAL := help

# ADR-0019 Tier-1 verbs: help build test lint fmt check clean. `build` (alias of
# `wasm`), `test`, `lint` already exist below; help/fmt/check/clean are added
# here. Every recipe runs inside the toolchain image (host needs only make+podman).
help:
	@echo "entity-browser-rust — make + podman (host needs only make + podman)"
	@echo
	@echo "  build    WASM debug build → dist/ (alias of wasm; conventional entry)"
	@echo "  test     native unit + peer-integration suite, in-container"
	@echo "  lint     cargo clippy, in-container (read-only)"
	@echo "  fmt      cargo fmt, in-container (writes)"
	@echo "  check    lint + test (the green gate)"
	@echo "  clean    remove dist/ and the toolchain image"
	@echo
	@echo "  wasm / wasm-release / serve / build-serve / tauri-run / e2e-worker"
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
	podman run --rm $(PODMAN_RUN_CAPS) \
		-v $(PARENT):/src/entity-systems:z \
		-v $(CARGO_CACHE):/usr/local/cargo/registry:z \
		-v $(TRUNK_CACHE):/root/.cache:z \
		-e CARGO_TARGET_DIR=$(TARGET_DIR) \
		$(EXTRA_RUN_ENV) \
		-w /src/entity-systems/$(notdir $(CURDIR)) \
		$(IMAGE) \
		sh -c '$(1)'
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

# Lint, in-container.
lint: image
	$(call RUN,cargo clippy)

# Tier-1 fmt = autoformat (writes), in-container.
fmt: image
	$(call RUN,cargo fmt)

# Tier-1 check = the green gate (lint + test).
check: lint test

# Tier-1 clean = remove the host-visible build output (dist/) and the toolchain
# image. The cargo registry/target cache lives in a persistent named volume and
# is left intact (delete $(CARGO_CACHE) by hand for a full cold reset).
clean:
	rm -rf dist/ dist-publish/
	-podman rmi $(IMAGE)

# WASM debug build → dist/. Single bundle for both Direct (default)
# and Worker (`?worker=1`) modes — capability detection at boot picks
# Worker automatically when available and falls back to Direct on
# failure (Stage 1B).
wasm: image
	$(call RUN,trunk build --dist $(DIST) && ./tools/check-dist.sh $(DIST))

# Alias — `make build` is the conventional bare-box entry point across the repo group.
build: wasm

# WASM release build → dist/
wasm-release: image
	$(call RUN,trunk build --release --dist $(DIST) && ./tools/check-dist.sh $(DIST))

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

# E2E browser test (exercises Worker mode via `?worker=1`). Requires
# the Selenium-firefox container running on :4444 — see
# tools/e2e/README.md. The test prints the full captured browser
# console under --nocapture, so this is the primary diagnostic path
# without manual browser refresh.
e2e-worker: image
	# The e2e dist is built WITH `--features demo-apps`: the launcher→player
	# e2e (Phase 2h.2) needs a deterministic baked app (war/calculator) to
	# render + launch without a live origin. demo-apps is OFF in every
	# release/serve/tauri build — no fake apps are shipped (see Cargo.toml
	# [features]) — so this is the ONE build that bakes them.
	$(call RUN,trunk build --features demo-apps --dist $(DIST) && ./tools/check-dist.sh $(DIST))
	# --features e2e: the e2e_worker suite is `#![cfg(feature = "e2e")]`, so it
	# compiles to nothing (and `make test` stays bare-box green) UNLESS the
	# feature is on. This target turns it on; it needs Selenium on :4444.
	# --test-threads=1: the e2e tests share one Selenium session + http port,
	# so they must run serially (the main boot test + the multi-tab guard test).
	cargo test --features e2e --test e2e_worker -- --nocapture --test-threads=1

# Tauri desktop (size-optimized release WASM + debug backend, logs to stdout).
# Use this for development — stable WASM (no overflow panics). Right-click
# in the WebView → Inspect Element for the WebKit Inspector (Console, DOM,
# Performance, Memory). NOTE (C8): the release profile is now size-optimized
# (debug=false + wasm-opt -Oz), so the Inspector no longer source-maps Rust.
# To debug Rust here, set [profile.release] debug=true in Cargo.toml and
# index.html data-wasm-opt="0" temporarily.
#
# NOTE: Same unified bundle as `make wasm`; runtime boot detects Tauri
# and forces Direct mode (src/main.rs). WebKitGTK ≤ 2.52 lacks
# `WorkerNavigator.storage`, so worker-mode OPFS init HARD-FAILS there
# (it does not silently in-memory — build_async returns Err, which would
# round-trip and fall back to Direct+banner). We preemptively force Direct
# in Tauri to skip that wasted failed-worker spawn. Browser deployments get
# worker-mode automatically via capability detection. Tracked in
# WORKER-MODE-LIVING-DOC §3.6.
tauri: wasm-release
	# Re-embed the freshly-built frontend. `tauri::generate_context!()`
	# (src-tauri/src/lib.rs) reads ../dist at macro-expansion time, but
	# cargo's incremental compiler can't see that dependency — so when only
	# dist/ changed it leaves lib.rs "up to date" and embeds the STALE
	# frontend (you launch an old UI). We bypass Tauri's CLI asset pipeline
	# here (raw `cargo build`), so nothing else tracks it. Touching the
	# embedding source forces a re-expand + re-embed every build. Cheap:
	# this target always reruns wasm-release anyway, so it's never a no-op.
	$(call RUN,touch src-tauri/src/lib.rs && cd src-tauri && cargo build)
	@echo ""
	@echo "Built: ./src-tauri/target/debug/entity-browser-tauri"
	@echo "Launch on a desktop session (needs webkit2gtk + a display): make tauri-run"
	@echo "Inspector: right-click → Inspect Element in the WebView"

# Tauri — build and run in one step
tauri-run: tauri
	./src-tauri/target/debug/entity-browser-tauri

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
	./src-tauri/target/debug/entity-browser-tauri

# Serve whatever is currently in dist/ (no rebuild). Fast, but does NOT
# guarantee the bundle is current — use `make build-serve` when you need
# certainty you're serving the latest optimized build.
serve:
	@echo "  → http://localhost:$(PORT)   (override with: make serve PORT=8082)"
	python3 -m http.server $(PORT) --directory dist

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
	python3 -m http.server $(PORT) --directory dist

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
# publish-serve emits the deployment-config by DEFAULT, so the served SPA
# cold-boots into the published content (registers the publish peer's origin +
# lands on its home site) — otherwise the SPA shows only its own boot seed and
# your published sites appear missing. Turn it off with DEPLOY_CONFIG=0.
publish-serve: DEPLOY_CONFIG := 1
publish-serve: wasm
	$(snapshot_serve_dir)
	@mkdir -p $(PUBLISH_DATA_DIR)
	ENTITY_DATA_DIR=$(CURDIR)/$(PUBLISH_DATA_DIR) CARGO_TARGET_DIR=$(TARGET_DIR) cargo run --quiet --bin entity-browser -- publish $(SERVE_DIR) $(if $(INGEST),--ingest=$(INGEST),) $(if $(APPS_DIST),--ingest-apps=$(APPS_DIST),) --live=$(LIVE) $(if $(filter-out 0,$(DEPLOY_CONFIG)),--deployment-config,) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(SURFACE),--surface=$(SURFACE),) $(if $(WINDOW_TYPE),--window-type=$(WINDOW_TYPE),) $(if $(LOCKED),--locked,) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),) $(if $(DEMO_IDENTITY),--demo-identity,)
	@echo ""
	@echo "=== fresh build + published sites — serving on :$(PORT) (one origin, isolated $(SERVE_DIR)) ==="
	@echo "  ▶ live entity browser (SPA):  http://localhost:$(PORT)/"
	@echo "  ▶ static published sites:     http://localhost:$(PORT)/sites/   (banner → live)"
	@echo "  (hard-refresh once if an older build is cached)"
	@echo ""
	python3 -m http.server $(PORT) --directory $(SERVE_DIR)

.PHONY: native test lint wasm wasm-release wasm-measurement e2e-worker tauri tauri-run tauri-bundle tauri-bundle-run serve build-serve check-dist publish publish-bare publish-serve

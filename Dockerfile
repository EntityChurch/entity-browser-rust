# Toolchain-only image for the WASM browser build of egui-entity-core-rust.
#
# This repo is a Rust -> wasm32 browser app built with Trunk. The image
# carries ONLY the build toolchain (Rust, the wasm32 target, Trunk, and
# wasm-opt/binaryen); the source is bind-mounted at run time by the Makefile,
# which mounts the PARENT meta dir at /src/entity-systems so the sibling
# `entity-core-rust` workspace deps resolve.
#
# Pins:
#   - Rust 1.94.1            -> rust-toolchain.toml (channel)
#   - Trunk 0.21.14         -> mise.toml ("cargo:trunk")
#   - wasm32-unknown-unknown target -> rust-toolchain.toml / Cargo build target
#
# Trunk downloads its own wasm-bindgen-cli (version-matched to the project's
# wasm-bindgen crate) on first build, so we do NOT pin it here. wasm-opt is
# required for release builds (index.html data-wasm-opt="z"); we install a
# PINNED modern binaryen from upstream (NOT the distro — see below) so it is on
# PATH for `make wasm-release`.
#
# === WHY THE PINS BELOW ARE BY DIGEST AND NOT BY VERSION =====================
#
# This image IS the build environment, and a build environment is named by a
# digest or it is not named at all. `PROPOSAL-EXTENSION-PACKAGE` §8 states the
# rule for the whole ecosystem — *pin the build environment by digest, not a
# tag* — and `build_env_digest` is the field that consumes it. An image built
# `FROM` a moving tag has nothing to put in that field: two builds a week apart
# are two different environments wearing one name.
#
# This is not theoretical here. The binaryen block below is a monument to it:
# a toolchain component moved under us and silently mis-compiled the release
# bundle in exactly one WebView engine. That was caught by a human opening the
# desktop app, not by any gate.
#
# Four inputs are pinned by content, and one is pinned only by version:
#   - the base image      -> FROM ...@sha256
#   - the binaryen tarball-> sha256 verified before it is unpacked
#   - the trunk tarball   -> sha256 verified before it goes on PATH. DOWNLOADED,
#                            not compiled, and that is a measurement rather than
#                            a preference — see the trunk block.
#   - tauri-cli           -> an exact version, not a caret range. Still COMPILED,
#                            because it measured byte-identical and replacing a
#                            reproducible step is churn.
#
# === WHAT `make image-verify` MEASURED, 2026-09-11 ==========================
#
# The first version of this header predicted the drift source and named the
# WRONG ONE: it said *"Debian package versions float, so two builds can still
# differ"*, and pointed at snapshot.debian.org. Two cold `--no-cache` builds
# then differed, and the diff says otherwise:
#
#   dpkg-query over both images -> BYTE-IDENTICAL, 788 packages, same versions.
#
# **Debian did not move. Two things did, and neither is a package:**
#
#   1. `cargo install` builds in a RANDOMLY NAMED temp dir and `trunk` bakes the
#      path into its binary — measured as `/tmp/cargo-installV9VSzp` in one arm
#      and `/tmp/cargo-installGcHvbX` in the other. `cargo-tauri` and
#      `cargo-xwin` carry ZERO such references and were byte-identical, which is
#      why "all cargo installs drift" would also have been the wrong story.
#      Fixed below with a fixed `--target-dir`.
#   2. The package manager's own TIMESTAMPED LOGS — `/var/log/dpkg.log` and
#      `/var/log/apt/history.log` — plus `/var/cache/ldconfig/aux-cache`.
#      `/etc/ld.so.cache` was identical. Fixed below by removing them in the
#      same layer that creates them.
#
# === SECOND ROUND, 2026-09-11: THE FIX WORKED AND THE IMAGE IS STILL NOT ======
# === REPRODUCIBLE — 4 layers, and a FULL FILESYSTEM DIFF names every file =====
#
# Two more cold builds with both fixes in: **still divergent**, at layers 7, 9,
# 11 and 12 (the webkit apt block, both `cargo install`s, the windows apt block).
#
# **The first round's conclusions all held** — zero `cargo-install` paths remain
# in `trunk`, `cargo-tauri` and `cargo-xwin` are byte-identical, and all 44,438
# files of the cargo registry hash the same. The fixes fixed what they named.
#
# **What settled the rest was not another hypothesis: it was hashing every file
# in both images and diffing the manifests.** 83,041 files per arm, and exactly
# NINE differ. Two of them (`/etc/hostname`, `/etc/hosts`) are podman's runtime
# mounts and are not image content at all — verified by reading `/etc/hostname`
# inside a container and finding the container id. The other seven:
#
#   - `/etc/machine-id` + `/var/lib/dbus/machine-id` — **a random UUID that
#     dbus's postinst generates at install time.** Not a log, not a cache: the
#     image was carrying a fresh identity every build. This is the one no amount
#     of pinning versions would ever have found.
#   - `/var/log/alternatives.log` — the same class as `dpkg.log`, and **the first
#     round's list enumerated two of the three.**
#   - `/var/cache/fontconfig/*.cache-8` — built at package-install time.
#   - `/usr/local/cargo/.global-cache` — cargo's GC tracking database, which
#     records when each artifact was last used.
#   - `/usr/local/cargo/bin/trunk` — see the trunk block below. The only one of
#     the seven that is not bookkeeping, and the only one not fixed here.
#
# ⭐ **The transferable half, and it is sharper than the first round's: a LAYER
# DIGEST tells you a layer moved; only a file-level diff tells you what moved.**
# The first round reasoned from layer sizes and a targeted `dpkg-query`, got a
# real answer, and missed three files of the same class plus a random UUID. One
# `find | xargs sha256sum` per arm, ~4 minutes, named all seven with no theory in
# between — and six of the seven were fixable in the same sitting.
#
# **Six of seven fixed. The image is still not reproducible, and that is the
# honest state:** `trunk` is compiled, and compiling is where it leaks.
#
# What was already deterministic, and is the evidence the two content pins
# above do their job: the five base-image layers and the binaryen layer were
# byte-identical across both arms, as was `rustup component add` — downloading
# a prebuilt artifact is reproducible; COMPILING one is where it leaks.
#
# **The transferable half: a guess about which input drifts is worth nothing
# next to one run of the two-build diff.** Both of the real causes are
# bookkeeping — a temp path and a log file — and neither would have been found
# by pinning versions harder.
#
# Still unpinned by content and deliberately so: Debian package VERSIONS. They
# did not move between two builds minutes apart, which is not evidence they are
# stable over weeks. snapshot.debian.org remains the route if a later
# `image-verify` names them. Do not read today's green as that question settled.
#
# Bumping any of these: change the version AND the digest together, in one
# commit. A version bumped without its digest fails the build, which is the
# intended behaviour — the pin is the thing that makes the bump deliberate.
#
# rust:1.94.1-bookworm, resolved 2026-09-11.
FROM rust:1.94.1-bookworm@sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55

# binaryen provides wasm-opt, which Trunk invokes for release builds
# (index.html `data-wasm-opt="z"`). Debug builds skip it.
#
# We DELIBERATELY do NOT use Debian Bookworm's `binaryen` apt package: it is
# pinned at binaryen 108 (2022), which MIS-OPTIMIZES the wasm reference-types
# funcref table under -Oz. The resulting module throws
#   "RangeError: WebAssembly.Table.prototype.grow could not grow the table"
# in JavaScriptCore — i.e. WebKitGTK, the Tauri desktop WebView — so the whole
# release frontend fails to boot there. SpiderMonkey/Firefox tolerates the same
# bundle, which is why the headless-Firefox e2e never caught it (regression
# introduced when the build moved into this container, commit 8e5d8a8;
# diagnosed later). Pin a modern binaryen release from upstream instead.
#
# TRACKED VERSION — bump deliberately, keep in sync with any host wasm-opt:
ARG BINARYEN_VERSION=version_119
# ...and the CONTENT of each arch's tarball, because a version is a name and a
# name is not what broke us last time. A GitHub release asset can be replaced;
# a sha256 cannot. Computed 2026-09-11 against version_119.
ARG BINARYEN_SHA256_x86_64=716bcf9f5f36a6f466239fbb09a925eeaf54c46411ccefac979ec649e7c06d2d
ARG BINARYEN_SHA256_aarch64=537b0c137afcde45ea42df72e46fc19738c60af8ca78be9319967eefb6f8bcf6
# Arch-resolved, NOT hardcoded x86_64: the release matrix builds linux-arm64 on
# a native aarch64 runner, where the x86_64 tarball installs "successfully" and
# then dies at `wasm-opt --version` (exec format error). Upstream publishes
# aarch64-linux beside x86_64-linux, so resolve from `uname -m` and let the
# version check below prove the right one landed.
#
# Downloaded to a file and verified BEFORE unpacking — the previous form piped
# curl straight into tar, which cannot check anything: by the time a mismatch
# could be noticed the bytes are already on disk and on PATH.
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64)  BINARYEN_ARCH=x86_64;  BINARYEN_SHA256="$BINARYEN_SHA256_x86_64" ;; \
      aarch64) BINARYEN_ARCH=aarch64; BINARYEN_SHA256="$BINARYEN_SHA256_aarch64" ;; \
      *) echo "no pinned binaryen build for $(uname -m)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/binaryen.tar.gz \
        "https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN_VERSION}/binaryen-${BINARYEN_VERSION}-${BINARYEN_ARCH}-linux.tar.gz"; \
    echo "${BINARYEN_SHA256}  /tmp/binaryen.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/binaryen.tar.gz -C /opt; \
    rm -f /tmp/binaryen.tar.gz; \
    ln -s "/opt/binaryen-${BINARYEN_VERSION}/bin/wasm-opt" /usr/local/bin/wasm-opt; \
    wasm-opt --version   # fail the image build early if the pin/URL is wrong

# Tauri v2 desktop-build deps (the `make tauri` / `tauri-run` native WebView
# backend under src-tauri/). Tauri 2 links webkit2gtk-4.1 + the GTK / libsoup /
# appindicator / rsvg stack on Linux; without these the native `cargo build`
# in src-tauri fails to find the system libraries.
#
# fonts-noto-color-emoji: this base image ships NO emoji/symbol font at all —
# WebKitGTK renders any pictographic glyph (🐞 🎮) as a tofu box, and several
# "media control" symbols the UI uses (⏸ ⏭) are ALSO emoji-set codepoints under
# Unicode, so they tofu too even though they look like plain monochrome icons.
# Plain arrows (↻ ⇄) survive because they're covered by the base Sans fallback
# GTK pulls in transitively — this is exactly the class of "green in Firefox
# (has a system emoji font) ≠ works in WebKitGTK (this container has none)" gap.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev \
        libjavascriptcoregtk-4.1-dev \
        libsoup-3.0-dev \
        libgtk-3-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libssl-dev \
        pkg-config \
        python3 \
        fonts-noto-color-emoji \
        # **WebKitGTK's WebRTC backend is GStreamer, and these are the elements
        # it needs**: `webrtcbin` + `dtls` + `srtp` live in -plugins-bad and the
        # ICE agent (`nice`) in gstreamer1.0-nice. Measured before adding: the
        # image carried -plugins-base and -plugins-good only
        # (`libgstrtpmanager.so` and none of the rest).
        #
        # **INERT TODAY, and kept deliberately.** Debian's and Fedora's
        # libwebkit2gtk-4.1 both ship WITHOUT the WebRTC bindings —
        # `RTCPeerConnection` is `undefined` on a secure origin whatever
        # `enable-webrtc` is set to (probed on 2.50.6 and 2.50.5; see
        # `src-tauri/src/lib.rs::enable_webview_webrtc` for the table). So this
        # is the half of the requirement that IS ours, recorded so the next
        # person does not have to re-bisect which half was missing. Same class
        # as the missing emoji font above: a runtime dependency of the WEBVIEW,
        # invisible to `cargo build`.
        gstreamer1.0-plugins-bad \
        gstreamer1.0-nice \
    # Install-time bookkeeping. EVERY ITEM HERE WAS MEASURED by a full
    # filesystem diff of two cold builds, never reasoned about — nothing in a
    # build image reads any of it, and leaving it makes two identical builds
    # differ.
    #
    # `machine-id` is the one that is not a log, and it is the interesting one:
    # dbus's postinst GENERATES A RANDOM UUID and writes it here, so the image
    # carried a different identity every build. Truncated rather than deleted —
    # an empty file is the documented "uninitialized, generate at first boot"
    # state, where an absent one makes some tooling error.
    #
    # ⚠ THIS IS AN ENUMERATION AND THAT IS ITS WEAKNESS (AP44). It cannot be
    # hoisted into one cleanup layer at the end: a final `rm` whiteouts the files
    # but leaves the EARLIER layers — and therefore the image digest — still
    # divergent, so it has to run in the layer that creates them. The structural
    # backstop is `make image-verify` itself, and it earned that description on
    # 2026-09-11 by catching `/var/log/alternatives.log`, which the first cut of
    # this very block enumerated two thirds of.
    && rm -rf /var/lib/apt/lists/* \
        /var/log/dpkg.log /var/log/apt /var/log/alternatives.log \
        /var/cache/ldconfig/aux-cache /var/cache/fontconfig \
    && : > /etc/machine-id \
    && { [ -e /var/lib/dbus/machine-id ] && : > /var/lib/dbus/machine-id || true; }

# The wasm browser target + the components the repo's rust-toolchain.toml
# pins (clippy, rustfmt). Installing them at image-build time means that when
# Trunk runs `cargo metadata` (which makes rustup honor rust-toolchain.toml),
# the toolchain is already complete and rustup does NOT try to sync/download
# components mid-build (that runtime sync fails in this image's rustup layout).
RUN rustup component add clippy rustfmt \
    && rustup target add wasm32-unknown-unknown

# Trunk 0.21.14, the version the project expects (mise.toml).
#
# === WE DOWNLOAD TRUNK, WE DO NOT COMPILE IT — and that is a measured decision ==
#
# It was `cargo install --locked trunk@0.21.14` until 2026-09-11. Two rounds of
# `make image-verify` say why it is not:
#
#   ROUND 1. `cargo install` builds in a RANDOMLY NAMED `/tmp/cargo-installXXXXXX`
#   and `trunk` baked the path into its binary (`…V9VSzp` vs `…GcHvbX`).
#   `cargo-tauri` and `cargo-xwin` carried ZERO such references and were
#   byte-identical — so "all cargo installs drift" would have been the wrong
#   story too. A fixed `--target-dir` removed that leak and it worked: zero
#   `cargo-install` paths remain in either arm.
#
#   ROUND 2. **`trunk` still differed, and the cause was not a path.** Same
#   31,954,896 bytes, no embedded temp dir, sorted string tables differing only
#   in binary noise, and **4,042,923 of 32 MB different, spread through the
#   file.** That is the signature of non-determinism INSIDE the compile. Pinning
#   inputs harder cannot reach it, and neither can a third guess at a flag.
#
# **So the fix is the one this file's own evidence already pointed at:
# downloading a prebuilt artifact is reproducible; COMPILING one is where it
# leaks.** The five base layers, the binaryen tarball and `rustup component add`
# were byte-identical from the very first run — every one of them downloads. Every
# layer that ever drifted here compiles something. This is the binaryen pattern
# applied to the one tool that needed it, and it also takes a 3m11s compile out
# of the image build as a side effect, which is not the reason to do it.
#
# Checksums computed 2026-09-11 against the published v0.21.14 release archives
# (each contains exactly one file, `trunk`). Bump the version and BOTH sums in
# one commit; a version moved without its sum fails the build, deliberately.
#
# `cargo-tauri` and `cargo-xwin` deliberately keep compiling: they are already
# byte-identical, and replacing a measured-reproducible step is churn. If a later
# `image-verify` names one of them, it gets this same treatment.
ARG TRUNK_VERSION=v0.21.14
ARG TRUNK_SHA256_x86_64=f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b
ARG TRUNK_SHA256_aarch64=b1d8e60e454f7fc182d9a4d95d1506ffbae947d8ba90f8f6f02da93b60f980f9
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64)  TRUNK_ARCH=x86_64;  TRUNK_SHA256="$TRUNK_SHA256_x86_64" ;; \
      aarch64) TRUNK_ARCH=aarch64; TRUNK_SHA256="$TRUNK_SHA256_aarch64" ;; \
      *) echo "no pinned trunk build for $(uname -m)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/trunk.tar.gz \
        "https://github.com/trunk-rs/trunk/releases/download/${TRUNK_VERSION}/trunk-${TRUNK_ARCH}-unknown-linux-gnu.tar.gz"; \
    echo "${TRUNK_SHA256}  /tmp/trunk.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/trunk.tar.gz -C /usr/local/bin trunk; \
    rm -f /tmp/trunk.tar.gz; \
    chmod 0755 /usr/local/bin/trunk; \
    trunk --version   # fail the image build early if the pin/URL is wrong

# AppImage release-bundling toolchain (`make appimage` — Tauri's bundler packs
# the binary + webkit2gtk + every runtime lib into ONE portable file that runs
# on other Linux hosts with no dev toolchain). The bundler shells out to:
# desktop-file-utils (validate), librsvg2-bin (rsvg-convert, icon rasterize),
# patchelf (rpath fixups), squashfs-tools + zsync (AppImage assembly). FUSE is
# NOT available in the container, so the `appimage` target runs the downloaded
# appimagetool/linuxdeploy via APPIMAGE_EXTRACT_AND_RUN=1.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        desktop-file-utils \
        librsvg2-bin \
        patchelf \
        squashfs-tools \
        zsync \
    # Install-time bookkeeping. EVERY ITEM HERE WAS MEASURED by a full
    # filesystem diff of two cold builds, never reasoned about — nothing in a
    # build image reads any of it, and leaving it makes two identical builds
    # differ.
    #
    # `machine-id` is the one that is not a log, and it is the interesting one:
    # dbus's postinst GENERATES A RANDOM UUID and writes it here, so the image
    # carried a different identity every build. Truncated rather than deleted —
    # an empty file is the documented "uninitialized, generate at first boot"
    # state, where an absent one makes some tooling error.
    #
    # ⚠ THIS IS AN ENUMERATION AND THAT IS ITS WEAKNESS (AP44). It cannot be
    # hoisted into one cleanup layer at the end: a final `rm` whiteouts the files
    # but leaves the EARLIER layers — and therefore the image digest — still
    # divergent, so it has to run in the layer that creates them. The structural
    # backstop is `make image-verify` itself, and it earned that description on
    # 2026-09-11 by catching `/var/log/alternatives.log`, which the first cut of
    # this very block enumerated two thirds of.
    && rm -rf /var/lib/apt/lists/* \
        /var/log/dpkg.log /var/log/apt /var/log/alternatives.log \
        /var/cache/ldconfig/aux-cache /var/cache/fontconfig \
    && : > /etc/machine-id \
    && { [ -e /var/lib/dbus/machine-id ] && : > /var/lib/dbus/machine-id || true; }

# Tauri v2 CLI (drives `cargo tauri build --bundles appimage`). Matches the
# `tauri = "2"` crate in src-tauri/Cargo.toml.
#
# EXACT, not '^2'. A caret range is a floating input: two builds of this file a
# week apart got different CLIs, silently, and the image could not say which one
# it carried. 2.11.4 is what the pre-pin image resolved to, so this pin changes
# nothing today and stops it moving tomorrow. Bump deliberately.
RUN cargo install --locked --target-dir /tmp/ct tauri-cli --version 2.11.4 \
    && rm -rf /tmp/ct /usr/local/cargo/.global-cache

# --- Windows cross-build toolchain (`make dist DIST_OS=windows`) -------------
# Produces a real Windows installer FROM LINUX, so a contributor with only
# make + podman can build the Windows artifact without owning a Windows box.
# Verified: `Entity Browser_0.8.0_x64-setup.exe`, 9.4M.
#
#   clang + lld  — compile and link the PE (rustc's MSVC target needs an
#                  MSVC-compatible driver; lld-link is the linker)
#   nsis         — makensis, which Tauri drives to assemble the installer
#   cargo-xwin   — supplies Microsoft's CRT + Windows SDK headers/libs
#
# The SDK itself is NOT baked in: cargo-xwin downloads it on first use (under
# Microsoft's licence, which the Makefile makes you accept explicitly) into
# ~/.cache/cargo-xwin — which rides the same persistent cache mount as trunk's,
# so it downloads once, not per build.
#
# NOTE this buys `_setup.exe` (NSIS) only. `.msi` is WiX and tauri-bundler
# gates it behind `#[cfg(target_os = "windows")]`; it cannot be cross-built,
# which is why the release workflow still runs a real Windows runner.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang \
        lld \
        llvm \
        nsis \
    # Install-time bookkeeping. EVERY ITEM HERE WAS MEASURED by a full
    # filesystem diff of two cold builds, never reasoned about — nothing in a
    # build image reads any of it, and leaving it makes two identical builds
    # differ.
    #
    # `machine-id` is the one that is not a log, and it is the interesting one:
    # dbus's postinst GENERATES A RANDOM UUID and writes it here, so the image
    # carried a different identity every build. Truncated rather than deleted —
    # an empty file is the documented "uninitialized, generate at first boot"
    # state, where an absent one makes some tooling error.
    #
    # ⚠ THIS IS AN ENUMERATION AND THAT IS ITS WEAKNESS (AP44). It cannot be
    # hoisted into one cleanup layer at the end: a final `rm` whiteouts the files
    # but leaves the EARLIER layers — and therefore the image digest — still
    # divergent, so it has to run in the layer that creates them. The structural
    # backstop is `make image-verify` itself, and it earned that description on
    # 2026-09-11 by catching `/var/log/alternatives.log`, which the first cut of
    # this very block enumerated two thirds of.
    && rm -rf /var/lib/apt/lists/* \
        /var/log/dpkg.log /var/log/apt /var/log/alternatives.log \
        /var/cache/ldconfig/aux-cache /var/cache/fontconfig \
    && : > /etc/machine-id \
    && { [ -e /var/lib/dbus/machine-id ] && : > /var/lib/dbus/machine-id || true; } \
    && rustup target add x86_64-pc-windows-msvc \
    && cargo install --locked --target-dir /tmp/ct cargo-xwin \
    && rm -rf /tmp/ct /usr/local/cargo/.global-cache

# === NORMALISE THE BOOKKEEPING, LAST ========================================
#
# The last four drift sources `make image-verify` named, and every one of them
# is DERIVED DATA that no build step reads — an identity minted at random, two
# caches built from files that are themselves identical, and a log of when
# things were installed. None is a package, a version, or a toolchain byte.
#
#   /etc/machine-id, /var/lib/dbus/machine-id — the dbus postinst mints a random
#     id per build. EMPTIED rather than deleted: an empty machine-id is the
#     documented "generate one at boot" signal, where a missing file is an error
#     for anything that reads it.
#   /var/cache/fontconfig/* — built from the installed font files, which ARE
#     identical across arms; the cache embeds scan order and mtimes. Regenerated
#     on first use, so the emoji fonts this image installs deliberately (see the
#     webkit block) still resolve — the cache is derived, the fonts are not.
#   /var/log/alternatives.log — update-alternatives writes its own timestamped
#     log, separate from dpkg's. It was missed in the first pass precisely
#     because it is a THIRD log in a THIRD place.
#
# **This runs LAST on purpose.** Every one of these is written by a postinst
# somewhere above, so a normalisation placed mid-file cleans state that later
# layers then recreate. The cost of getting that wrong is silent: the image still
# works and `image-verify` still reds, with the fix apparently applied.
#
# **And it is a layer of its own on purpose** — folding it into the last apt
# block would tie "normalise the image" to "install the Windows toolchain", so
# removing that block would silently take this with it.
RUN set -eux; \
    : > /etc/machine-id; \
    [ -e /var/lib/dbus/machine-id ] && : > /var/lib/dbus/machine-id || true; \
    rm -rf /var/cache/fontconfig/* /var/log/alternatives.log

WORKDIR /src/entity-systems

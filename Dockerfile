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
FROM rust:1.94.1-bookworm

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
# Arch-resolved, NOT hardcoded x86_64: the release matrix builds linux-arm64 on
# a native aarch64 runner, where the x86_64 tarball installs "successfully" and
# then dies at `wasm-opt --version` (exec format error). Upstream publishes
# aarch64-linux beside x86_64-linux, so resolve from `uname -m` and let the
# version check below prove the right one landed.
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64)  BINARYEN_ARCH=x86_64 ;; \
      aarch64) BINARYEN_ARCH=aarch64 ;; \
      *) echo "no pinned binaryen build for $(uname -m)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN_VERSION}/binaryen-${BINARYEN_VERSION}-${BINARYEN_ARCH}-linux.tar.gz" \
        | tar -xz -C /opt \
    && ln -s "/opt/binaryen-${BINARYEN_VERSION}/bin/wasm-opt" /usr/local/bin/wasm-opt \
    && wasm-opt --version   # fail the image build early if the pin/URL is wrong

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
    && rm -rf /var/lib/apt/lists/*

# The wasm browser target + the components the repo's rust-toolchain.toml
# pins (clippy, rustfmt). Installing them at image-build time means that when
# Trunk runs `cargo metadata` (which makes rustup honor rust-toolchain.toml),
# the toolchain is already complete and rustup does NOT try to sync/download
# components mid-build (that runtime sync fails in this image's rustup layout).
RUN rustup component add clippy rustfmt \
    && rustup target add wasm32-unknown-unknown

# Trunk pinned to the version the project expects (mise.toml). --locked keeps
# Trunk's own dependency resolution reproducible.
RUN cargo install --locked trunk@0.21.14

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
    && rm -rf /var/lib/apt/lists/*

# Tauri v2 CLI (drives `cargo tauri build --bundles appimage`). Matches the
# `tauri = "2"` crate in src-tauri/Cargo.toml.
RUN cargo install --locked tauri-cli --version '^2'

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
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-pc-windows-msvc \
    && cargo install --locked cargo-xwin

WORKDIR /src/entity-systems

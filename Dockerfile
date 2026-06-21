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
RUN curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN_VERSION}/binaryen-${BINARYEN_VERSION}-x86_64-linux.tar.gz" \
        | tar -xz -C /opt \
    && ln -s "/opt/binaryen-${BINARYEN_VERSION}/bin/wasm-opt" /usr/local/bin/wasm-opt \
    && wasm-opt --version   # fail the image build early if the pin/URL is wrong

# Tauri v2 desktop-build deps (the `make tauri` / `tauri-run` native WebView
# backend under src-tauri/). Tauri 2 links webkit2gtk-4.1 + the GTK / libsoup /
# appindicator / rsvg stack on Linux; without these the native `cargo build`
# in src-tauri fails to find the system libraries.
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

WORKDIR /src/entity-systems

#!/bin/sh
# Execute the upstream wasm-worker-protocol crate's `#[wasm_bindgen_test]`
# suites (e.g. `v11_wire_shape`) — the wire-shape / additivity assertions that
# guard "absent v11 fields == v10 behaviour".
#
# WHY THIS EXISTS. That crate is `#![cfg(target_arch = "wasm32")]`, so the
# assertions compile to nothing on native and `make test` cannot run them; they
# need a `wasm_bindgen_test` runner. This repo's charter forbids Node in the
# *build* toolchain, and the build image ships none — but nothing stops us from
# pulling a throwaway Node into an *ephemeral container* purely to execute a
# test. That is what this does. (`make wasm-test-protocol` wraps it in podman.)
#
# GOTCHA (load-bearing): the tests are node-configured (no `run_in_browser`),
# and Debian's node 18 is TOO OLD for wasm-bindgen 0.2.117's externref/GC glue —
# it dies with a V8 fatal ("Stacktrace: ptr1=…") right after "running N tests",
# before any assertion, which reads like a test failure but is not. Node 22
# runs all suites green. So we fetch node 22 explicitly rather than apt's node.
#
# Runs INSIDE the build image (has cargo + the wasm32 target). Needs network to
# fetch node + the wasm-bindgen-test-runner. Writes only to /tmp — neither
# repo's tree is touched. Verified 2026-08-03: v11_wire_shape 8/8 green.
set -e

NODE_VER=${NODE_VER:-22.11.0}
WB_VER=${WB_VER:-0.2.117}   # MUST match the crate's wasm-bindgen (Cargo.lock)
CRATE=${CRATE:-entity-wasm-worker-protocol}
WORKSPACE=${WORKSPACE:-/src/entity-systems/entity-core-rust}

echo "=== fetch node v$NODE_VER (official static) ==="
cd /tmp
curl -sSL -o node.tar.xz "https://nodejs.org/dist/v$NODE_VER/node-v$NODE_VER-linux-x64.tar.xz"
tar xf node.tar.xz
export PATH="/tmp/node-v$NODE_VER-linux-x64/bin:$PATH"
node --version

echo "=== fetch wasm-bindgen-test-runner $WB_VER (musl prebuilt) ==="
curl -sSL -o wb.tar.gz "https://github.com/rustwasm/wasm-bindgen/releases/download/$WB_VER/wasm-bindgen-$WB_VER-x86_64-unknown-linux-musl.tar.gz"
tar xzf wb.tar.gz
WB="/tmp/wasm-bindgen-$WB_VER-x86_64-unknown-linux-musl"

echo "=== cargo test -p $CRATE --target wasm32-unknown-unknown ==="
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="$WB/wasm-bindgen-test-runner"
export CARGO_TARGET_DIR=/tmp/wtarget   # keep artifacts out of both repo trees
cd "$WORKSPACE"
cargo test -p "$CRATE" --target wasm32-unknown-unknown -- --nocapture

//! Escape-hatch budget — the D15 caller-grep, mechanized.
//!
//! The Direct/Worker arm split is the browser's physics (a peer's SDK
//! runs on the main thread *or* in a Web Worker; Worker mode is the
//! durable-storage default). The `Peers` router (`get_entity`,
//! `tree_listing`, `dispatch_write`, `put_and_wait`, `execute`, `query`,
//! `count`) is the cross-arm surface every feature should use. A handful
//! of **Direct-only L0 escape hatches** (`direct_peer_context`,
//! `direct_peer_shared`) still exist for genuinely-Direct paths
//! (bootstrap, shell identity ops) — they are `Result`/`Option`-typed and
//! log a break-glass warning when reached through on a Worker peer, but
//! the durable defense is this: **the set of files allowed to reach for
//! one is pinned here.** A new reach-through turns this test red, forcing
//! the author to either route via the `Peers` API or *consciously* add
//! the call site to the allowlist with a justification.
//!
//! This is the mechanical form of charter discipline D15 / anti-pattern
//! AP4, and mirrors the upstream `L1_WORKER_MIRRORED_SURFACE ≡
//! REQUEST_VARIANT_NAMES` compile-time surface pin.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `peers.rs` is the hatches' definition home (defs + internal self-calls
/// + the `#[cfg(test)] test_seed_ctx` accessor) — not a caller budget.
const HOME_FILE: &str = "src/peers.rs";

/// Files allowed to call `Peers::direct_peer_context` (Direct-only L0
/// `PeerContext`). The arm-aware *write* seeds (site-mode, origins,
/// content-site demo) used to be here; they now route through the blessed
/// `Peers::seed_write` and no longer touch the hatch. The sole remaining
/// caller is genuinely Direct-only and degrades gracefully on Worker.
const DIRECT_PEER_CONTEXT_ALLOWED: &[&str] = &[
    "src/views/shell/binding.rs", // shell identity ops: Direct-only, return "not supported on Worker-arm peer"
];

/// Files allowed to call `Peers::direct_peer_shared` (Direct-only
/// `PeerShared`). All primary-bootstrap / engine-startup, `Option`-handled.
const DIRECT_PEER_SHARED_ALLOWED: &[&str] = &[
    "src/listener_state.rs",      // listener writer: primary shared state at startup
    "src/app.rs",                 // engine startup + fetch wiring (primary)
    "src/views/shell/model.rs",   // shell native ws-listen test/server path
];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// All `.rs` files under `src/`, as paths relative to the crate root
/// (forward-slash), paired with their contents.
fn rust_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    let mut stack = vec![src_dir()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir src/") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let body = std::fs::read_to_string(&path).expect("read source");
                out.push((rel, body));
            }
        }
    }
    out
}

/// Files (relative, forward-slash) containing a *call* of `needle`
/// (`.<needle>(`), ignoring comment lines and the definition home.
fn callers_of(needle: &str) -> BTreeSet<String> {
    let pat = format!(".{needle}(");
    let mut files = BTreeSet::new();
    for (rel, body) in rust_sources() {
        if rel == HOME_FILE {
            continue;
        }
        for line in body.lines() {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with('*') {
                continue;
            }
            if line.contains(&pat) {
                files.insert(rel.clone());
                break;
            }
        }
    }
    files
}

fn assert_budget(hatch: &str, allowed: &[&str]) {
    let live = callers_of(hatch);
    let allowed: BTreeSet<String> = allowed.iter().map(|s| s.to_string()).collect();

    let unexpected: Vec<_> = live.difference(&allowed).cloned().collect();
    let stale: Vec<_> = allowed.difference(&live).cloned().collect();

    assert!(
        unexpected.is_empty(),
        "NEW reach-through for `{hatch}` in: {unexpected:?}\n\
         The Direct/Worker arm split means this L0 hatch is silently wrong on \
         Worker-arm (the browser default) peers. Route via the `Peers` router \
         (get_entity / tree_listing / dispatch_write / put_and_wait) instead. \
         If the call is genuinely Direct-only and degrades gracefully on Worker, \
         add the file to the allowlist in tests/escape_hatch_budget.rs WITH a \
         one-line justification."
    );
    assert!(
        stale.is_empty(),
        "Allowlist for `{hatch}` lists files that no longer call it: {stale:?}\n\
         Prune them from tests/escape_hatch_budget.rs to keep the budget honest."
    );
}

#[test]
fn peer_context_or_default_is_deleted_forever() {
    // The double footgun (panic-on-Worker + silent default-to-primary,
    // anti-pattern AP2) was deleted in §L1. It must never return.
    let callers = callers_of("peer_context_or_default");
    assert!(
        callers.is_empty(),
        "`peer_context_or_default` is back in: {callers:?}. It is anti-pattern \
         AP2 (silent default-to-primary) and panicked on the Worker arm — there \
         is no legitimate use. Use the `Peers` router, an explicit \
         `primary_as_direct()` guard, or `#[cfg(test)] test_seed_ctx` for tests."
    );
}

#[test]
fn direct_peer_context_callers_are_budgeted() {
    assert_budget("direct_peer_context", DIRECT_PEER_CONTEXT_ALLOWED);
}

#[test]
fn direct_peer_shared_callers_are_budgeted() {
    assert_budget("direct_peer_shared", DIRECT_PEER_SHARED_ALLOWED);
}

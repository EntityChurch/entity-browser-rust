//! Verification harness for the compute / bootstrap / bundle shell-verb
//! landing in `entity-core-rust/bindings/shell/`.
//!
//! **Status as of creation:** verbs not yet authored
//! upstream. This harness runs today and reports per-verb wiring state;
//! each `*_is_wired` test SKIPS-with-PRINT until the corresponding verb
//! appears in `dispatcher::VERBS`. Once core-Rust lands the verb
//! authorship arc per the core-Rust shell-verb guide,
//! the same tests flip from skip → assertion and validate the §8
//! output-shape contract from that guide.
//!
//! **Discipline:** every verb the guide promises has at least one test
//! here. If you add a verb in the guide, add a test here. If a test
//! here has no matching verb in the guide, delete the test.
//!
//! Run with: `cargo test --test shell_verb_validation`.

use entity_shell::dispatcher;

// ---------------------------------------------------------------------------
// Phase 0 — dispatcher wiring. These tests run TODAY and tell us which
// verbs are in flight.
// ---------------------------------------------------------------------------

/// Source of truth for what we're waiting on. Every verb-token below
/// must appear in `dispatcher::VERBS` once the landing arc completes.
const EXPECTED_TOP_LEVEL_VERBS: &[&str] = &["compute", "bootstrap", "inspect"];

/// Subcommand inventory (for documentation; sub-dispatch is internal to
/// the top-level verb handler per the guide §4).
const EXPECTED_COMPUTE_SUBS: &[&str] = &["eval", "install", "uninstall", "list", "show"];
const EXPECTED_BOOTSTRAP_SUBS: &[&str] = &["status", "export", "import"];
const EXPECTED_INSPECT_SUBS: &[&str] = &[
    "chain", "under", "errors", "entity", "dump", "find", "help",
];

#[test]
fn compute_top_level_verb_registered() {
    print_wiring_state("compute");
}

#[test]
fn bootstrap_top_level_verb_registered() {
    print_wiring_state("bootstrap");
}

#[test]
fn guide_verb_inventory_matches_dispatcher() {
    // If this fails, either the guide grew a verb the dispatcher
    // doesn't have yet (skip until landing), or the dispatcher grew
    // a verb the guide doesn't enumerate (extend EXPECTED_TOP_LEVEL_VERBS).
    let landed: Vec<&str> = EXPECTED_TOP_LEVEL_VERBS
        .iter()
        .copied()
        .filter(|v| dispatcher::VERBS.contains(v))
        .collect();
    let pending: Vec<&str> = EXPECTED_TOP_LEVEL_VERBS
        .iter()
        .copied()
        .filter(|v| !dispatcher::VERBS.contains(v))
        .collect();

    println!(
        "shell-verb landing status: landed={:?}, pending={:?}",
        landed, pending
    );

    // No assertion — pure status print. The per-verb tests below
    // skip-with-print when they're not in VERBS yet.
}

// ---------------------------------------------------------------------------
// Phase 1 — once a verb is registered, exercise the §8 output contract
// from the guide. Tests SKIP-with-PRINT today, ASSERT once VERBS contains
// the verb.
// ---------------------------------------------------------------------------

#[test]
fn compute_list_dispatches_on_empty_peer() {
    if !skip_if_pending("compute") {
        return;
    }

    let (mut shell, binding, action_sink) = stub_session();
    let mut spawned = false;
    let result = dispatcher::dispatch(
        "compute list",
        &mut shell,
        &binding,
        None,
        &action_sink,
        |_fut| {
            // `compute list` is async (an L1 query so it works on the
            // Worker arm); the verb hands us a producer task. We only
            // assert it dispatches here — driving the query needs a live
            // peer/runtime, covered by the worker E2E.
            spawned = true;
        },
    );

    let output = result
        .expect("dispatcher should recognize `compute`")
        .expect("compute list should not error on empty peer");

    assert!(
        matches!(output, entity_shell::VerbOutput::Dispatch(_)),
        "compute list is now async → Dispatch; got {:?}",
        output
    );
    assert!(spawned, "compute list should hand a producer task to spawn");
}

#[test]
fn bootstrap_status_returns_info_rows() {
    if !skip_if_pending("bootstrap") {
        return;
    }

    let (mut shell, binding, action_sink) = stub_session();
    let result = dispatcher::dispatch(
        "bootstrap status",
        &mut shell,
        &binding,
        None,
        &action_sink,
        |_fut| {},
    );

    let output = result
        .expect("dispatcher should recognize `bootstrap`")
        .expect("bootstrap status should not error on fresh peer");

    // Structural assertion only — variant must be Info per guide §8.
    // Row content is end-to-end behavior covered by sdk_consumer_validation
    // against a real PeerContext. The stub binding here returns the
    // trait's default empty status; the verb's job is to wrap that as
    // Info (possibly with a "not bootstrapped" row prepended), not to
    // synthesize state the binding didn't provide.
    assert!(
        matches!(output, entity_shell::VerbOutput::Info(_)),
        "guide §8 says `bootstrap status` returns Info; got {:?}",
        output
    );
}

#[test]
fn compute_eval_usage_error_on_missing_path() {
    if !skip_if_pending("compute") {
        return;
    }

    let (mut shell, binding, action_sink) = stub_session();
    let result = dispatcher::dispatch(
        "compute eval",
        &mut shell,
        &binding,
        None,
        &action_sink,
        |_fut| {},
    )
    .expect("dispatcher should recognize `compute`");

    assert!(
        matches!(&result, Err(e) if e.code == entity_shell::ErrorCode::Usage),
        "guide §7 requires Usage error when expr-path is missing; got {:?}",
        result
    );
}

#[test]
fn bootstrap_unknown_arg_errors() {
    if !skip_if_pending("bootstrap") {
        return;
    }

    let (mut shell, binding, action_sink) = stub_session();
    let result = dispatcher::dispatch(
        "bootstrap --bogus-flag",
        &mut shell,
        &binding,
        None,
        &action_sink,
        |_fut| {},
    )
    .expect("dispatcher should recognize `bootstrap`");

    assert!(
        matches!(&result, Err(e) if e.code == entity_shell::ErrorCode::Usage),
        "guide §7 requires Usage error on unknown flag; got {:?}",
        result
    );
}

#[test]
fn compute_subcommand_taxonomy_matches_guide() {
    if !skip_if_pending("compute") {
        return;
    }

    // Each subcommand should be at-least-reachable (returns either
    // Ok(...) or a typed ShellError, never None — None would mean the
    // top-level verb didn't dispatch).
    let (mut shell, binding, action_sink) = stub_session();
    for sub in EXPECTED_COMPUTE_SUBS {
        let line = format!("compute {}", sub);
        let result = dispatcher::dispatch(
            &line,
            &mut shell,
            &binding,
            None,
            &action_sink,
            |_fut| {},
        );
        assert!(
            result.is_some(),
            "subcommand `compute {}` must be reachable from the top-level dispatch — guide §4",
            sub
        );
    }
}

#[test]
fn bootstrap_subcommand_taxonomy_matches_guide() {
    if !skip_if_pending("bootstrap") {
        return;
    }

    let (mut shell, binding, action_sink) = stub_session();
    for sub in EXPECTED_BOOTSTRAP_SUBS {
        let line = format!("bootstrap {}", sub);
        let result = dispatcher::dispatch(
            &line,
            &mut shell,
            &binding,
            None,
            &action_sink,
            |_fut| {},
        );
        assert!(
            result.is_some(),
            "subcommand `bootstrap {}` must be reachable from the top-level dispatch — guide §4",
            sub
        );
    }
}

#[test]
fn inspect_top_level_verb_registered() {
    print_wiring_state("inspect");
}

#[test]
fn inspect_subcommand_taxonomy_matches_guide() {
    if !skip_if_pending("inspect") {
        return;
    }

    let (mut shell, binding, action_sink) = stub_session();
    for sub in EXPECTED_INSPECT_SUBS {
        let line = format!("inspect {}", sub);
        let result = dispatcher::dispatch(
            &line,
            &mut shell,
            &binding,
            None,
            &action_sink,
            |_fut| {},
        );
        assert!(
            result.is_some(),
            "subcommand `inspect {}` must be reachable from the top-level dispatch",
            sub,
        );
    }
}

#[test]
fn inspect_help_lists_all_subs_in_consumer_path() {
    // Dispatcher → verb-parser → help_op via the same path the shell
    // window uses. Surfaces an end-to-end reachability smoke check.
    if !skip_if_pending("inspect") {
        return;
    }
    let (mut shell, binding, action_sink) = stub_session();
    let result = dispatcher::dispatch(
        "inspect help",
        &mut shell,
        &binding,
        None,
        &action_sink,
        |_fut| {},
    )
    .expect("dispatcher should recognize `inspect`")
    .expect("inspect help should not error");

    let text = match result {
        entity_shell::VerbOutput::Listing { sections } => sections
            .into_iter()
            .flat_map(|s| s.entries)
            .collect::<Vec<_>>()
            .join("\n"),
        other => panic!("expected Listing output from inspect help; got {:?}", other),
    };
    for sub in ["chain", "under", "errors"] {
        assert!(
            text.contains(sub),
            "inspect help must mention sub-op `{}` so users can discover it",
            sub,
        );
    }
}

// Substrate-driven end-to-end exercise (`inspect chain` finding a
// real marker bound at the canonical path) lives in
// `tests/inspectability_e2e.rs` via the real `PeersBinding` — the
// stub binding here can't drive a substrate. The two tests above
// cover the dispatcher-reachability + help-text-discoverability
// claims that ARE provable from a stub.

#[test]
fn help_text_mentions_landed_verbs() {
    // The shell crate's own `help_covers_every_dispatched_verb` test
    // enforces help mirroring — this is a consumer-side smoke that the
    // dispatcher state matches what we tell users in the help cheatsheet.
    let (mut shell, binding, action_sink) = stub_session();
    let result = dispatcher::dispatch(
        "help",
        &mut shell,
        &binding,
        None,
        &action_sink,
        |_fut| {},
    )
    .expect("help is always wired")
    .expect("help never errors");

    let text = match result {
        entity_shell::VerbOutput::Info(rows) => rows
            .iter()
            .map(|r| r.value.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        other => panic!("help must return Info; got {:?}", other),
    };

    for verb in EXPECTED_TOP_LEVEL_VERBS {
        if dispatcher::VERBS.contains(verb) {
            assert!(
                text.contains(verb),
                "help text must mention landed verb `{}` per guide §6",
                verb
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true if the verb is registered and tests should run; prints
/// a SKIP message and returns false if pending.
fn skip_if_pending(verb: &str) -> bool {
    if dispatcher::VERBS.contains(&verb) {
        true
    } else {
        println!(
            "SKIP — `{}` not yet in dispatcher::VERBS; flip once the core-Rust shell verb lands",
            verb
        );
        false
    }
}

/// Print whether a top-level verb is wired yet. The wiring tests are
/// always-on status prints — they never fail; the per-verb shape tests
/// flip from skip to assert when wiring lands.
fn print_wiring_state(verb: &str) {
    let landed = dispatcher::VERBS.contains(&verb);
    println!(
        "wiring `{}`: {}",
        verb,
        if landed { "LANDED" } else { "pending (core-Rust shell verb not yet wired)" }
    );
}

/// Minimum-viable session: stub binding (defaults-to-error for SDK
/// methods), shell at root, no-op action sink. Sufficient for shape /
/// usage / wiring assertions; not for happy-path SDK round-trips
/// (those go through `tests/sdk_consumer_validation.rs` against real
/// `PeerContext`).
fn stub_session() -> (entity_shell::Shell, StubBinding, StubActionSink) {
    let shell = entity_shell::Shell::with_wd("p1", "/p1/");
    let binding = StubBinding;
    let action_sink = StubActionSink;
    (shell, binding, action_sink)
}

struct StubBinding;

impl entity_shell::PeerBinding for StubBinding {
    fn peer_id(&self) -> &str {
        "p1"
    }
    fn primary_peer_id(&self) -> String {
        "p1".into()
    }
    fn peer_ids(&self) -> Vec<String> {
        vec!["p1".into()]
    }
    fn connected_peers(&self) -> Vec<String> {
        Vec::new()
    }
    fn peer_label(&self, _: &str) -> Option<String> {
        None
    }
    fn tree_listing(&self, _: &str, _: &str) -> Vec<entity_shell::TreeListingEntry> {
        Vec::new()
    }
    fn get_entity(&self, _: &str, _: &str) -> Option<entity_shell::EntityRead> {
        None
    }
    // All SDK-tier methods use the trait's default `not supported` impls —
    // that's enough for the wiring / shape / usage-error tests above.
    // When core-Rust extends the trait per the guide §2, this stub will
    // keep compiling unchanged (defaults absorb new methods).
}

struct StubActionSink;

impl entity_shell::AppActionSink for StubActionSink {
    fn submit(&self, _request: entity_shell::ShellRequest) {}
}

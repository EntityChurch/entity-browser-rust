//! **Census — no e2e gate may assert a PHASE-2 fact behind a PHASE-1 barrier.**
//!
//! Since the two-phase boot became the default (2026-09-02), `wait_for_boot`
//! polls for *"Frame loop started"*, which arms after phase 1's **local** reads
//! (258–287 ms measured). Everything that depends on `/entity-deployment.json`
//! — the origins adoption, the supersession persist/revalidate, the startup
//! surface, the `boot_diagnostics` routing mirror — runs in a **spawned**
//! `boot_phase2` behind a bounded network fetch. A gate that calls
//! `wait_for_boot` and then reads a phase-2 marker out of a one-shot
//! `capture_log` is therefore racing a fetch it cannot see.
//!
//! **Both directions of that race cost something, which is why this is a census
//! and not a style note:**
//!
//! * a **positive** assertion (*"the document was applied"*) reds intermittently
//!   and names the product — `a_home_the_user_chose_is_not_overwritten_by_the_
//!   deployments_declaration` failed 1 unfiltered run in 3 with *"the cold boot
//!   never applied the document"*, with the browser log ending at *"boot phase
//!   2: DEFERRED"*;
//! * a **negative** assertion (*"the document was NOT applied"*) goes **green
//!   for the wrong reason**, silently, forever. That one was live too.
//!
//! **Why a text census and not a compile-time guard.** The barrier is an
//! ordering property between two statements in an async block; nothing in the
//! type system expresses it. This is `AGENTS.md`'s standing fallback — *if you
//! cannot make it structural, make the census fail, and then falsify the
//! census* — and it is the same shape as `tests/window_hydration_census.rs`.
//!
//! **It runs in `make test`, not behind `--features e2e`,** because it reads
//! `tests/e2e_worker.rs` as *text*. That is deliberate: the e2e suite compiles
//! to nothing without the feature, so a gate that needed it would be one more
//! thing only an 11-minute Selenium run could check.

use std::path::Path;

/// Log lines emitted **only** from `boot_phase2` or deeper.
///
/// **Asserted by count below**, so adding a phase-2 log line that a gate keys on
/// forces someone to put it here rather than quietly inheriting the race. Each
/// entry names where it is emitted, because "is this phase 2?" is the whole
/// question and a bare string cannot answer it.
const PHASE2_MARKERS: &[(&str, &str)] = &[
    // `session_config::apply_to`, reached only after the document is in hand.
    ("deployment-config: applied", "boot_phase2 → deployment document adoption"),
    // `peer_supersession::persist`, discovered FROM the document (phase 1 only
    // `load`s what a previous boot wrote — see AGENTS.md on why that asymmetry
    // is the whole reason the surface spawn is phase-2 work).
    ("peer-supersession: recorded", "boot_phase2 → supersession persist"),
    // The re-key WARN, from the same comparison.
    ("the domain now publishes under a DIFFERENT identity", "boot_phase2 → re-key detection"),
];

/// How far back a barrier may sit and still count. Generous: these gates are
/// long, and a barrier two screens up is still a barrier. Being generous is the
/// safe direction — it makes the census under-report, never over-report.
const LOOKBACK_LINES: usize = 60;

fn suite() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e_worker.rs");
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("the census cannot read {}: {e}", p.display()))
}

/// A line that *asserts on* a marker, as opposed to defining or documenting one.
///
/// Without this the census flags `const APPLIED: &str = "…"` and its own doc
/// comments — and a census that cries wolf on the definitions gets an allowlist
/// bolted on, which is how a census stops meaning anything.
fn is_assertion_site(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("//") || t.starts_with("///") || t.starts_with("const ") {
        return false;
    }
    line.contains("iter()") || line.contains(".any(")
}

/// Walk back from `idx` to the nearest barrier. `Ok` = a phase-2 barrier,
/// `Err(what)` = the phase-1 one (or nothing), which is the defect.
fn barrier_before(lines: &[&str], idx: usize) -> Result<(), &'static str> {
    let start = idx.saturating_sub(LOOKBACK_LINES);
    for line in lines[start..=idx].iter().rev() {
        if line.contains("wait_for_phase2") {
            return Ok(());
        }
        if line.contains("wait_for_boot") {
            return Err("wait_for_boot — that is PHASE 1");
        }
    }
    Err("no barrier at all")
}

/// **The census.** Every phase-2 marker assertion sits behind `wait_for_phase2`.
#[test]
fn no_e2e_gate_asserts_a_phase_2_fact_behind_a_phase_1_barrier() {
    let src = suite();
    let lines: Vec<&str> = src.lines().collect();

    let mut checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if !is_assertion_site(line) {
            continue;
        }
        let Some((marker, emitted_by)) =
            PHASE2_MARKERS.iter().find(|(m, _)| line.contains(m)).copied()
        else {
            continue;
        };
        checked += 1;
        if let Err(what) = barrier_before(&lines, i) {
            offenders.push(format!(
                "  tests/e2e_worker.rs:{}\n    asserts {marker:?} (emitted by {emitted_by})\n    \
                 nearest barrier: {what}\n    → {}",
                i + 1,
                line.trim()
            ));
        }
    }

    // **Anti-vacuity.** A census that matched nothing would pass forever — and
    // it would pass most loudly on the day someone renamed a marker, which is
    // exactly when it is needed.
    assert!(
        checked >= PHASE2_MARKERS.len(),
        "the census found only {checked} assertion site(s) across {} markers — it is not \
         matching the suite any more. A renamed log line or a changed assertion style will do \
         this, and a census that matches nothing passes forever.",
        PHASE2_MARKERS.len()
    );

    assert!(
        offenders.is_empty(),
        "{} e2e assertion(s) read a PHASE-2 fact behind a PHASE-1 barrier.\n\n{}\n\n\
         `wait_for_boot` returns when the frame loop arms — phase 1, local reads only. \
         Everything driven by /entity-deployment.json runs in a spawned `boot_phase2` behind \
         a bounded fetch. Put `wait_for_phase2(&client, 30_000).await?;` after the \
         `wait_for_boot`.\n\n\
         Note which way each of these fails: a POSITIVE assertion reds intermittently and \
         blames the product; a NEGATIVE one ({:?}) goes green for the wrong reason and stays \
         that way.",
        offenders.len(),
        offenders.join("\n\n"),
        "!log.iter().any(|l| l.contains(\"deployment-config: applied\"))",
    );
}

/// **The census's own falsifier — both directions, on synthetic input.**
///
/// `window_hydration_census` shipped green while blind, because it matched a
/// `#[cfg(test)]` helper of the same name; the rule earned from that
/// (`AGENTS.md`, AP44) is that *a census you have not falsified reports what you
/// hoped*. So the predicate is exercised against text that must fail and text
/// that must pass, rather than trusted because the real file is clean today.
#[test]
fn the_census_catches_a_missing_barrier_and_accepts_a_present_one() {
    let bad: Vec<&str> = vec![
        "        wait_for_boot(&client, 30_000).await?;",
        "        let cold = capture_log(&client).await?;",
        "        assert!(cold.iter().any(|l| l.contains(\"deployment-config: applied\")));",
    ];
    assert!(
        barrier_before(&bad, 2).is_err(),
        "the census accepts a phase-2 assertion guarded only by wait_for_boot — it would have \
         passed on the very defect it exists for"
    );

    let good: Vec<&str> = vec![
        "        wait_for_boot(&client, 30_000).await?;",
        "        wait_for_phase2(&client, 30_000).await?;",
        "        let cold = capture_log(&client).await?;",
        "        assert!(cold.iter().any(|l| l.contains(\"deployment-config: applied\")));",
    ];
    assert!(
        barrier_before(&good, 3).is_ok(),
        "the census rejects a correctly-guarded assertion — it would force every author to \
         work around it, which is how a census gets deleted"
    );

    // A bare `const` definition is not an assertion site; flagging it is what
    // gets an allowlist bolted on.
    assert!(!is_assertion_site("    const APPLIED: &str = \"deployment-config: applied\";"));
    assert!(!is_assertion_site("    /// asserts \"deployment-config: applied\" via iter().any()"));
    assert!(is_assertion_site("        x.iter().any(|l| l.contains(\"deployment-config: applied\"))"));
}

/// The marker table is pinned by **count**, so a fourth phase-2 log line that a
/// gate keys on cannot be added without someone classifying it.
#[test]
fn the_phase_2_marker_table_is_pinned() {
    assert_eq!(
        PHASE2_MARKERS.len(),
        3,
        "a phase-2 marker was added or removed. Adding one is the moment to ask whether the \
         gates keying on it wait for phase 2 — which is the entire question this census exists \
         to keep asked."
    );
    for (marker, emitted_by) in PHASE2_MARKERS {
        assert!(!marker.is_empty() && !emitted_by.is_empty(), "a marker row must say where it comes from");
    }
}

//! **The origin registry's adopt and un-name are a PAIR, and nothing structural
//! makes them one.** — `DESIGN-RESILIENCE…` §1.1f item 3.
//!
//! `origins::adopt_deployment_origin` writes a row per origin the deployment
//! document declares. Until 2026-09-07 nothing ever removed one, so a peer a
//! domain stopped hosting kept a registered origin that 404s on every visit —
//! in the browse-all roster, in the retry ladder, and in Doctor's
//! `fetch-failure-by-peer`. `origins::unname_withdrawn_origins` is the paired
//! removal (D9), and the two have to run together: adopting without sweeping is
//! the pre-fix behaviour, and sweeping without adopting judges a listing against
//! rows the same boot was about to write.
//!
//! **Why this is a census and not a structure.** The honest fix is one function
//! doing both halves, so a caller cannot have one without the other (AP44 —
//! *if the rule needs the word "every", the structure has to enforce it*). It is
//! not taken here because the adopt loop's per-entry `expand_origin` is
//! WASM-only and three e2e gates key on the exact log lines that loop emits
//! (`registered deployment-config origin` × `Seeded`/`Updated`), so folding it
//! is a change whose blast radius needs a Selenium run to clear. Recorded as the
//! better fix rather than left implicit.
//!
//! **The census's own claim is falsified below, both directions**, on synthetic
//! input — because *a census you have not falsified reports what you hoped*, and
//! the two most load-bearing censuses in this repo shipped vacuous for days.

const ADOPT: &str = "adopt_deployment_origin(";
const UNNAME: &str = "unname_withdrawn_origins(";

fn app_source() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app.rs"))
        .expect("src/app.rs is readable from the test's manifest dir")
}

/// Every non-test call of the adopt has an un-name in the same function body.
///
/// "The same function body" is approximated by *the same `if let Some(dc)` /
/// document-gated region*, which the census expresses the only way a text check
/// honestly can: both calls must appear, and the un-name must appear **after**
/// the adopt. Ordering is not decoration — judging the listing before the adopt
/// loop has written this boot's declared rows withdraws a row that is about to
/// be re-seeded.
fn audit(source: &str) -> Result<(usize, usize), String> {
    let adopts: Vec<usize> = source.match_indices(ADOPT).map(|(i, _)| i).collect();
    let unnames: Vec<usize> = source.match_indices(UNNAME).map(|(i, _)| i).collect();
    if adopts.is_empty() {
        return Err(format!(
            "no call to `{ADOPT}` found at all. Either the boot no longer registers \
             deployment-declared origins — in which case this census is measuring a \
             subsystem that moved and must move with it — or the spelling changed and \
             this check has been silently passing."
        ));
    }
    if unnames.is_empty() {
        return Err(format!(
            "{} call(s) to `{ADOPT}` and NONE to `{UNNAME}`. The registry only ever \
             grows: a publisher the deployment stops declaring keeps a registered \
             origin that 404s forever (§1.1f item 3). Add the paired sweep after the \
             adopt loop, gated on a document having been read this boot.",
            adopts.len()
        ));
    }
    let first_adopt = adopts[0];
    if !unnames.iter().any(|u| *u > first_adopt) {
        return Err(format!(
            "`{UNNAME}` is called BEFORE `{ADOPT}`. The sweep judges the registry \
             against the document's declared set, so running it first withdraws rows \
             this same boot is about to write."
        ));
    }
    Ok((adopts.len(), unnames.len()))
}

#[test]
fn the_boot_that_adopts_declared_origins_also_un_names_withdrawn_ones() {
    match audit(&app_source()) {
        Ok((adopts, unnames)) => {
            println!("  origin reconcile: {adopts} adopt site(s), {unnames} un-name site(s)");
        }
        Err(e) => panic!("src/app.rs: {e}"),
    }
}

/// **The census's own falsifier — both directions, on synthetic input.**
///
/// Run against strings rather than the real file, so it keeps meaning something
/// when `src/app.rs` is correct. A census that only ever sees a passing input
/// has never demonstrated it can fail.
#[test]
fn the_census_catches_an_unpaired_adopt_and_a_reversed_order() {
    let paired = "adopt_deployment_origin(a); unname_withdrawn_origins(b);";
    assert!(audit(paired).is_ok(), "the paired shape must pass");

    let unpaired = "adopt_deployment_origin(a);";
    let err = audit(unpaired).expect_err("an adopt with no sweep must fail");
    assert!(err.contains("NONE to"), "wrong diagnosis: {err}");

    let reversed = "unname_withdrawn_origins(b); adopt_deployment_origin(a);";
    let err = audit(reversed).expect_err("a sweep before the adopt must fail");
    assert!(err.contains("BEFORE"), "wrong diagnosis: {err}");

    let neither = "nothing to see here";
    let err = audit(neither).expect_err("a file with no adopt at all must fail LOUDLY");
    assert!(
        err.contains("silently passing"),
        "a census whose subject vanished must say so rather than pass: {err}"
    );
}

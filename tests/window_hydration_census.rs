//! Window-state hydration census — AP41's class gate, mechanized.
//!
//! # The question this exists to answer
//!
//! *"If we add a new window model, how do we know it is protected?"*
//!
//! Before this file the answer was **we don't**. `WindowView::hydrate_durable`
//! is called by `WindowManager::spawn` for every window, which makes the *call*
//! structural — but its default is a no-op, so a new window that reads its
//! persisted state at construction and keeps the result gets no protection at
//! all unless its author happens to know to override the hook. That is AP44's
//! shape one level up: correctness that every future author has to remember.
//!
//! AP44's own rule says what to do about it: **if you cannot make it
//! structural, make the census fail.** This is the census, and it is the same
//! instrument AP42 landed on after getting its own census wrong —
//! `window_state_path` is what makes a surface exposed, *not* the spelling of
//! its state type, and the table asserts its own length so an omission fails
//! instead of passing quietly.
//!
//! # What it checks
//!
//! Every directory under `src/views/` whose sources mention `window_state_path`
//! persists per-window state, and therefore has to answer one question: **when
//! the construction-time read comes back empty for reasons of timing rather
//! than of truth, what corrects it?** There are exactly two acceptable answers,
//! and each is asserted rather than taken on the table's word:
//!
//! * [`Care::Hydrates`] — it overrides `hydrate_durable`. Asserted by the
//!   presence of `fn hydrate_durable` in that directory.
//! * [`Care::ReReads`] — it never retains, because it reads the tree afresh
//!   every time it needs the value (the `SettingsModel` shape: self-healing,
//!   because the next frame's read is authoritative). Asserted by the presence
//!   of the named accessor **and the absence of a `fn initialize`**, which is
//!   this codebase's marker for a cached construction read.
//!
//! # Why the axis is RETENTION, not persistence — measured 2026-08-31
//!
//! Eleven window surfaces persist state; only **eight** were ever in AP41's
//! inventory, and the difference is not arbitrary. `theme_editor`, `games` and
//! `programs` persist per-window state and are *not* in the class, because each
//! reads through a per-call accessor (`read_state`, `view_state`, and an inline
//! read inside `render_dom`) instead of caching into a field at construction.
//! A cold read that answers nothing is corrected by the next frame.
//!
//! **That is a property of today's code, not a guarantee** — nothing stops
//! someone turning `games::view_state` into a cached field for a frame budget,
//! and the moment they do, `games` joins the class silently. Which is the
//! second reason this file exists: the `ReReads` rows are pinned, so making one
//! of them retain turns this test red instead of shipping.
//!
//! [AP41, AP42, AP44, D15, `window.rs` `WindowView::hydrate_durable`,
//!  `tests/escape_hatch_budget.rs` (the same instrument, for the arm hatches)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How a window surface that persists per-window state stays correct when the
/// construction-time read is empty for reasons of timing.
#[derive(Debug, Clone, Copy)]
enum Care {
    /// Overrides `WindowView::hydrate_durable` — an authoritative,
    /// subscription-independent read corrects the cold one (AP41's repair).
    Hydrates,
    /// Never retains: reads the tree afresh every time, so there is nothing to
    /// correct. Carries the accessor's name so the claim is checkable.
    ReReads(&'static str),
}

/// **Every** `src/views/` surface that persists per-window state, and what
/// keeps it correct. A new window persisting state is not in this table and
/// therefore fails — which is the entire point.
///
/// Adding a row is a decision, not a formality: pick `Hydrates` and override
/// the hook, or pick `ReReads` and be sure the surface genuinely re-reads. Do
/// not add a row to make a red test green.
const CARE: &[(&str, Care)] = &[
    // -- The AP41 class: `initialize` caches a construction read. -----------
    ("chain_trace", Care::Hydrates),
    ("content_site", Care::Hydrates),
    ("entity_tree", Care::Hydrates),
    ("execute_console", Care::Hydrates),
    ("knowledge_base", Care::Hydrates),
    ("peer_connections", Care::Hydrates),
    ("query_console", Care::Hydrates),
    ("shell", Care::Hydrates),
    // -- Self-healing: re-read per use, so a cold read costs one frame. -----
    ("games", Care::ReReads("view_state")),
    ("programs", Care::ReReads("render_dom")),
    ("theme_editor", Care::ReReads("read_state")),
];

fn views_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/views")
}

/// Every `src/views/<surface>/` directory, mapped to the concatenated text of
/// its own `.rs` files. Immediate children only: a surface is a directory, and
/// a nested helper module belongs to the surface that contains it.
fn surface_sources() -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for entry in std::fs::read_dir(views_dir()).expect("read_dir src/views") {
        let path = entry.expect("dir entry").path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .expect("dir name")
            .to_string_lossy()
            .to_string();
        let mut body = String::new();
        let mut stack = vec![path];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).expect("read_dir surface") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    body.push_str(&std::fs::read_to_string(&p).expect("read source"));
                    body.push('\n');
                }
            }
        }
        out.insert(name, body);
    }
    out
}

/// Strip `//`-comment tails so a doc comment naming a function does not read as
/// a definition. Crude and deliberately so — it only has to stop the two false
/// positives this census can actually produce (`hydrate_durable` and
/// `initialize` are both discussed at length in doc comments here).
fn code_only(body: &str) -> String {
    body.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bodies of this surface's `impl WindowView for …` blocks, and **only**
/// those.
///
/// # This function exists because the first version of this gate had a false
/// negative, and an accident caught it rather than a falsification
///
/// The check was "does any source in this directory contain
/// `fn hydrate_durable`". `query_console`'s trait override was then deleted by
/// mistake — and the census stayed **green**, because its `model.rs` carries a
/// `#[cfg(test)] pub async fn hydrate_durable` test helper of the same name.
/// The gate was answering *"does this directory mention the function"* when the
/// question is *"does this window type override the trait method"*.
///
/// That is AP42's lesson repeating one file over: **falsify the gate, not just
/// the fix** — and it is why `every_hydrating_surface_actually_overrides_the_hook`
/// now carries a self-check (`the_override_check_does_not_match_a_helper_of_the_same_name`)
/// that proves this narrowing is load-bearing.
///
/// Relies on rustfmt's layout: an `impl` block ends at a `}` in column 0. That
/// is a real coupling, and it is why the self-check below exists rather than
/// this being trusted on its shape.
fn window_view_impls(body: &str) -> String {
    let code = code_only(body);
    let mut out = String::new();
    let mut rest = code.as_str();
    while let Some(start) = rest.find("impl WindowView for ") {
        let block = &rest[start..];
        let end = block.find("\n}").map(|i| i + 2).unwrap_or(block.len());
        out.push_str(&block[..end]);
        out.push('\n');
        rest = &block[end..];
    }
    out
}

/// The census: surfaces that persist per-window state. `window_state_path` is
/// the instrument, per AP42 — the *path* is what makes a surface exposed, not
/// the spelling of its state type. A literal grep for the type string missed
/// three surfaces the last time this was counted by hand.
fn persisting_surfaces() -> BTreeSet<String> {
    surface_sources()
        .into_iter()
        .filter(|(_, body)| body.contains("window_state_path"))
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn every_window_that_persists_state_is_classified_for_durable_hydration() {
    let census = persisting_surfaces();
    let table: BTreeSet<String> = CARE.iter().map(|(n, _)| n.to_string()).collect();

    let unclassified: Vec<_> = census.difference(&table).collect();
    assert!(
        unclassified.is_empty(),
        "these window surfaces persist per-window state but are not classified in CARE: {unclassified:?}\n\
         \n\
         A surface that writes to `window_state_path` reads it back somewhere, and that read is \
         empty on the Worker arm and racy on Direct-IDB (AP41). Decide which it is:\n\
         \n\
           Care::Hydrates          — override `WindowView::hydrate_durable` and correct the cold \
         read with `get_entity_async`. Mind the three traps: an errored round-trip is not an \
         answer, a change landing during the await is newer than it, and if the struct holds \
         session-only fields you must MERGE rather than assign.\n\
           Care::ReReads(\"fn\")     — only if the surface genuinely re-reads the tree every time \
         it needs the value, and therefore caches nothing to be wrong about."
    );

    let stale: Vec<_> = table.difference(&census).collect();
    assert!(
        stale.is_empty(),
        "CARE names surfaces that no longer persist per-window state: {stale:?} — \
         drop the rows rather than leaving the census overstated"
    );

    // Asserted so a forgotten row fails instead of passing quietly. AP42's
    // matrix earned this line: a falsification there FAILED to red because the
    // check silently skipped a case, and a length assertion is what caught it.
    assert_eq!(
        CARE.len(),
        census.len(),
        "the table and the census must be the same size"
    );
}

#[test]
fn every_hydrating_surface_actually_overrides_the_hook() {
    let sources = surface_sources();
    let mut missing = Vec::new();
    for (name, care) in CARE {
        let Care::Hydrates = care else { continue };
        let body = sources.get(*name).unwrap_or_else(|| {
            panic!("CARE names `{name}`, which is not a directory under src/views/")
        });
        // The trait impl specifically — NOT the directory. See
        // `window_view_impls` for the false negative that taught this.
        if !window_view_impls(body).contains("fn hydrate_durable") {
            missing.push(*name);
        }
    }
    assert!(
        missing.is_empty(),
        "classified `Care::Hydrates` but their `impl WindowView` blocks contain no \
         `fn hydrate_durable`: {missing:?} — the hook's default is a no-op, so an \
         un-overridden surface silently keeps whatever its cold read happened to return (AP41)"
    );
}

/// The gate's own falsifier, landed rather than performed once by hand.
///
/// `every_hydrating_surface_actually_overrides_the_hook` is only meaningful if
/// its search is scoped to the trait impl. Scoped to the directory it passes on
/// a surface with no override at all, because several models carry a
/// `#[cfg(test)] pub async fn hydrate_durable` helper — which is exactly how
/// the first version of this file went green on a genuinely broken tree.
///
/// This asserts the narrowing does work: a synthetic source with the helper but
/// no override must NOT read as overridden.
#[test]
fn the_override_check_does_not_match_a_helper_of_the_same_name() {
    let helper_only = r#"
impl SomethingElse for Window {
    #[cfg(test)]
    pub async fn hydrate_durable(&self, peers: &Peers) -> Hydration { todo!() }
}

impl WindowView for Window {
    fn title(&self) -> String { String::new() }
}
"#;
    assert!(
        !window_view_impls(helper_only).contains("fn hydrate_durable"),
        "a same-named helper OUTSIDE the trait impl must not read as an override — \
         if this fails, `every_hydrating_surface_actually_overrides_the_hook` is vacuous \
         and will stay green on a window that has no hook at all"
    );

    let real_override = r#"
impl WindowView for Window {
    fn title(&self) -> String { String::new() }
    fn hydrate_durable(&self, _peers: &Peers) {}
}
"#;
    assert!(
        window_view_impls(real_override).contains("fn hydrate_durable"),
        "a real override must be found — otherwise the check is vacuous the other way \
         and every surface reads as missing"
    );
}

#[test]
fn every_re_reading_surface_still_re_reads() {
    let sources = surface_sources();
    let mut broken = Vec::new();
    for (name, care) in CARE {
        let Care::ReReads(accessor) = care else {
            continue;
        };
        let body = code_only(sources.get(*name).unwrap_or_else(|| {
            panic!("CARE names `{name}`, which is not a directory under src/views/")
        }));
        if !body.contains(&format!("fn {accessor}")) {
            broken.push(format!("{name}: no `fn {accessor}` — the named accessor is gone"));
        }
        // `fn initialize` is this codebase's marker for a cached construction
        // read: it is the entry point all eight AP41 models share, and the
        // three self-healing surfaces have none. A surface that grows one has
        // almost certainly started retaining, and needs re-classifying rather
        // than a wider pattern here.
        if body.contains("fn initialize") {
            broken.push(format!(
                "{name}: gained a `fn initialize` — it may now RETAIN a construction read. \
                 Re-classify as `Care::Hydrates` and override the hook, or confirm it still \
                 re-reads and widen this check with a reason"
            ));
        }
    }
    assert!(
        broken.is_empty(),
        "surfaces claiming to re-read no longer look like they do: {broken:#?}"
    );
}

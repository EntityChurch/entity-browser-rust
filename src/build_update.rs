//! Is a newer build of the application available to this page?
//!
//! # Why this module exists (C7 / `DESIGN-CODE-AXIS` §1.4a)
//!
//! The *"a new version is available"* banner was raised from the service
//! worker registration's `updatefound` chain, which fires when the browser
//! finds **`/sw.js` byte-different** from the installed copy. `assets/sw.js` is
//! a `copy-file` asset whose bytes change only when we edit that one file, so
//! the signal and the question had almost nothing to do with each other.
//! Measured across two consecutive production deploys of one domain
//! (meta DevOps, 2026-09-06):
//!
//! | deploy | `sw.js` | banner | what changed for the user |
//! |---|---|---|---|
//! | `56c0921` → `ef3a7e1` | changed | fired | client code |
//! | `ef3a7e1` → `d9cc645` | byte-identical | **did not fire** | the entire re-key fix |
//!
//! So the deploy that mattered most notified nobody, and an `sw.js`-only change
//! would notify everybody about nothing. The operator reported the prompt as
//! unpredictable for weeks; it was deterministic and keyed to the wrong thing.
//!
//! # What the question actually is
//!
//! **Would reloading this page get you different application code?** Note the
//! framing — not *"what does the origin serve"*, which is a question this page
//! cannot always answer and does not need to. The banner's only button is
//! *Reload*, so the honest comparison is against **the freshest shell this
//! browser can obtain**, which is exactly what a reload would deliver:
//!
//! - Online, `sw.js`'s `networkFirst` fetches `/` with `cache: 'reload'`, so we
//!   compare against the origin.
//! - Offline, it falls back to the cached `/`. A reload would serve that same
//!   cached shell — so if it matches we are *right* to stay quiet, and if it is
//!   newer than the running page (a build fetched and cached but never reloaded
//!   into) we are *right* to prompt. Both arms are correct, which is why this
//!   goes **through** the service worker rather than trying to defeat it.
//!
//! Identity is the **bundle hash**, per `DESIGN-CODE-AXIS` §3.1 — never the
//! commit. Two docs-only commits produce byte-identical output, so keying on
//! the commit label would prompt for a no-op. That is not hypothetical: it is
//! the defect `fleet-probe` shipped and had fixed on 2026-09-02, where it
//! reported *"NOT uniform — 2 distinct builds"* for one build with two labels.
//!
//! # One expression, not a fourth
//!
//! *"Read a build id out of a shell"* already had three expressions —
//! [`crate::build_id::parse_bundle_hash`] (over a string), `sw.js`'s
//! `BUNDLE_HASH` regex, and `tools/build-stamp.sh`. This module adds **none**:
//! it calls `parse_bundle_hash` on the fetched document. A fourth spelling is
//! how that rule starts disagreeing with itself, which is the review finding
//! C15 exists to answer.
//!
//! # Tier
//!
//! This lives in the app, not in `index.html`. That file's tier is *what can
//! still help when the WASM is the broken thing* — the recovery console, the
//! boot-slot script, `__ENTITY_RECOVERY__` — and a prompt shown to someone in a
//! working session is not that. The old banner already admitted as much: its
//! own comment placed it at *"the same placement as the storage banner
//! (`storage_durability.rs`)"*, which is app-tier. Moving it is also the only
//! way its copy is ever translated, since `index.html` carries no i18n by
//! decision.
//!
//! `AUTO_RELOAD_ON_UPDATE` is untouched and stays `false` (S-5): faster
//! adoption of a broken build is not an improvement while C11 does not exist.

/// The shell the origin (or, offline, the cache) answered with.
///
/// Three states rather than an `Option<String>`, because *"nobody answered"*
/// and *"somebody answered 404"* decide differently and reading them as one is
/// the collapse AP40 names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginRead {
    /// 2xx, with the document body.
    Served(String),
    /// The origin answered, and not with a shell.
    NotOk(u16),
    /// Nothing answered inside the deadline.
    Unheard,
}

/// Why a check established nothing. Never folded into "you are up to date".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    /// No answer inside the deadline.
    Unheard,
    /// An answer, but not a shell — a 5xx, a redirect to a captive portal, a
    /// 404 from a misconfigured origin.
    OriginStatus(u16),
    /// A shell we could not find a bundle reference in. Different from
    /// [`Unheard`](Self::Unheard): the network is fine and the *document* is
    /// the thing we cannot read, which points at the build, not the link.
    OriginUnidentifiable,
}

/// What one check concluded. **Five outcomes, and only one of them prompts.**
///
/// They are kept apart for the reason every enum in this subsystem is: three of
/// these mean *we did not establish that you are current*, and rendering any of
/// them as silence-because-all-is-well is the failure mode the health surface
/// one screen over was rebuilt to avoid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The freshest shell available names the build we are running.
    Current,
    /// A different build is what a reload would deliver. **The only prompting
    /// outcome.**
    NewBuildAvailable { available: String },
    /// A different build is available and we are deliberately not offering it,
    /// because this page is a retained shell someone pinned. Prompting here
    /// would fight C10's rollback — which already has three ways out (a TTL,
    /// self-clear, and an attempt counter) and does not need a banner arguing
    /// with it.
    PinnedDeliberately { available: String },
    /// The check ran and concluded nothing.
    CouldNotCheck(Unknown),
    /// **This page cannot say what build it is**, so there is nothing to
    /// compare. Reported ahead of any network condition because it is a fact
    /// about this build that no amount of connectivity changes — an unstamped
    /// dev shell, or a bundle reference we could not parse.
    RunningBuildUnidentifiable,
}

impl UpdateCheck {
    /// One stable word for the log. Distinct per outcome and gated, so a sixth
    /// cannot quietly reuse one — the same rule `Verdict::label` carries.
    pub fn label(&self) -> &'static str {
        match self {
            UpdateCheck::Current => "current",
            UpdateCheck::NewBuildAvailable { .. } => "new-build-available",
            UpdateCheck::PinnedDeliberately { .. } => "pinned-deliberately",
            UpdateCheck::CouldNotCheck(_) => "could-not-check",
            UpdateCheck::RunningBuildUnidentifiable => "running-build-unidentifiable",
        }
    }

    /// **Does this put a banner in front of someone?** Exactly one outcome
    /// does. The method exists so no call site re-derives it and so the
    /// question has one answer in the tree — `Verdict::warrants_attention`'s
    /// lesson, applied on the way in rather than after.
    pub fn prompts(&self) -> bool {
        matches!(self, UpdateCheck::NewBuildAvailable { .. })
    }
}

/// Is this page the origin's canonical shell, or a retained one someone pinned?
///
/// **The same predicate `sw.js`'s `isCanonicalShell` applies**, and deliberately
/// expressed as a path test rather than by reading the boot-slot script's
/// `action` string. That script has nine action values; enumerating the three
/// that mean *"you are on a retained build"* would be a contract between two
/// files with no compiler in between — the shape `AGENTS.md` already flags for
/// `boot_diagnostics.rs` and the recovery console. Honouring a pin *navigates*
/// to `/builds/<id>/index.html`, so the path is the structural fact and needs
/// no agreement with anybody.
pub fn is_canonical_shell(pathname: &str) -> bool {
    pathname == "/" || pathname == "/index.html"
}

/// The whole decision, pure.
///
/// `running` is this page's bundle hash; `read` is what the fetch came back
/// with; `canonical` is [`is_canonical_shell`] over the current path.
pub fn decide(running: Option<&str>, read: &OriginRead, canonical: bool) -> UpdateCheck {
    // Ahead of every network arm on purpose. If we cannot name our own build
    // there is no comparison to make, and unlike a network condition that is
    // not going to resolve itself on the next attempt — so it is the more
    // useful of the two facts to log when both are true.
    let Some(running) = running else {
        return UpdateCheck::RunningBuildUnidentifiable;
    };

    let body = match read {
        OriginRead::Unheard => return UpdateCheck::CouldNotCheck(Unknown::Unheard),
        OriginRead::NotOk(s) => return UpdateCheck::CouldNotCheck(Unknown::OriginStatus(*s)),
        OriginRead::Served(b) => b,
    };

    let Some(available) = crate::build_id::parse_bundle_hash(body) else {
        return UpdateCheck::CouldNotCheck(Unknown::OriginUnidentifiable);
    };

    if available == running {
        return UpdateCheck::Current;
    }
    if !canonical {
        return UpdateCheck::PinnedDeliberately { available };
    }
    UpdateCheck::NewBuildAvailable { available }
}

// ── The wiring ──────────────────────────────────────────────────────────────

/// How long between checks, at minimum. Both triggers funnel through one gate,
/// so a tab being focused repeatedly costs one conditional GET per interval.
///
/// Five minutes rather than the hourly figure the standard guidance gives for
/// `reg.update()`: the complaint this closes is that a deploy went unnoticed,
/// and `/` is served `max-age=1, must-revalidate` on the live fleet (measured,
/// C16), so a repeat check is a 304 on a document already in hand.
#[cfg(target_arch = "wasm32")]
const MIN_INTERVAL_MS: f64 = 5.0 * 60.0 * 1000.0;

/// `?updatecheck=<ms>` — override [`MIN_INTERVAL_MS`].
///
/// **A test affordance nothing in the product sets**, the same shape and the
/// same justification as `?boothold=` and `?bootstall=`: without it the gate
/// for this feature has to wait out a five-minute interval, and the
/// alternative on the table was shipping the prompt with no browser gate at
/// all. It moves *when* a check happens and nothing about what it concludes,
/// so a test using it exercises the shipped decision.
#[cfg(target_arch = "wasm32")]
fn interval_ms() -> f64 {
    let Some(window) = web_sys::window() else {
        return MIN_INTERVAL_MS;
    };
    let Ok(search) = window.location().search() else {
        return MIN_INTERVAL_MS;
    };
    for pair in search.trim_start_matches('?').split('&') {
        if let Some(v) = pair.strip_prefix("updatecheck=") {
            if let Ok(ms) = v.parse::<f64>() {
                return ms;
            }
        }
    }
    MIN_INTERVAL_MS
}

/// Bounded like everything else that touches the network. **Not D23** — this
/// runs long after boot and blocks nothing — but a check with no deadline is a
/// timer that can outlive the reason it was set.
#[cfg(target_arch = "wasm32")]
const DEADLINE_MS: i32 = 5_000;

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Listener closures are OWNED here and never `Closure::forget()`-ed
    /// (D12 / AP1) — the `watchdog.rs` pattern. `last_ms` is the rate-limit
    /// gate both triggers share.
    static STATE: std::cell::RefCell<Option<State>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_arch = "wasm32")]
struct State {
    last_ms: f64,
    /// The live floor — `MIN_INTERVAL_MS`, or whatever `?updatecheck=` set.
    /// Held rather than re-read so the timer and the gate can never disagree.
    every_ms: f64,
    interval: i32,
    _onvis: wasm_bindgen::closure::Closure<dyn FnMut()>,
    _ontick: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for State {
    fn drop(&mut self) {
        if let Some(w) = web_sys::window() {
            w.clear_interval_with_handle(self.interval);
        }
    }
}

/// Start watching for a newer build. Idempotent; call it once, after boot.
///
/// **The first check is not immediate.** Nothing here depends on boot's result,
/// so it *could* run at once — but issuing a second request for the shell while
/// phase 2's own bounded read is in flight buys nothing and competes with it on
/// exactly the slow origins where boot is already struggling. The first check
/// lands one interval in, or on the first time the tab is brought back, and
/// that is soon enough for a prompt whose action the user takes at their
/// convenience.
#[cfg(target_arch = "wasm32")]
pub fn arm() {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    if STATE.with(|s| s.borrow().is_some()) {
        return; // already armed
    }
    let Some(win) = web_sys::window() else { return };

    // `visibilitychange` → the tab came back. The rate-limit gate is inside
    // `maybe_check`, so a user flipping between tabs does not issue a request
    // per flip.
    let onvis = Closure::wrap(Box::new(move || {
        if !document_hidden() {
            maybe_check();
        }
    }) as Box<dyn FnMut()>);
    if let Some(doc) = win.document() {
        let _ = doc
            .add_event_listener_with_callback("visibilitychange", onvis.as_ref().unchecked_ref());
    }

    // …and a plain interval, because a tab that is never blurred never fires
    // the event above and is precisely the long-lived session §1.4b is about.
    let ontick = Closure::wrap(Box::new(move || maybe_check()) as Box<dyn FnMut()>);
    let every_ms = interval_ms();
    let interval = win
        .set_interval_with_callback_and_timeout_and_arguments_0(
            ontick.as_ref().unchecked_ref(),
            every_ms as i32,
        )
        .unwrap_or(0);

    STATE.with(|s| {
        *s.borrow_mut() =
            Some(State { last_ms: now_ms(), every_ms, interval, _onvis: onvis, _ontick: ontick });
    });
    tracing::info!(
        interval_ms = every_ms,
        "update check armed — comparing this build against the shell a reload would deliver"
    );
}

/// The shared gate. Both triggers land here; at most one check per interval.
#[cfg(target_arch = "wasm32")]
fn maybe_check() {
    let due = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(st) = slot.as_mut() else { return false };
        let now = now_ms();
        if now - st.last_ms < st.every_ms {
            return false;
        }
        st.last_ms = now;
        true
    });
    if due {
        wasm_bindgen_futures::spawn_local(async {
            let outcome = check_once().await;
            report(&outcome);
        });
    }
}

/// One check, end to end. Public so a caller can force one.
#[cfg(target_arch = "wasm32")]
pub async fn check_once() -> UpdateCheck {
    let running = crate::build_id::current().bundle;
    let read = match crate::net::fetch_text_bounded("/", DEADLINE_MS).await {
        None => OriginRead::Unheard,
        Some(r) if !r.ok => OriginRead::NotOk(r.status),
        Some(r) => OriginRead::Served(r.text),
    };
    let canonical = web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .map(|p| is_canonical_shell(&p))
        .unwrap_or(true);
    decide(running.as_deref(), &read, canonical)
}

/// Log every outcome, prompt for exactly one.
///
/// **All five are logged**, including `Current`. A check whose only evidence of
/// having run is a banner that did not appear is indistinguishable from a check
/// that never ran — the same reason the health card's quiet state still says it
/// looked.
#[cfg(target_arch = "wasm32")]
fn report(outcome: &UpdateCheck) {
    match outcome {
        UpdateCheck::NewBuildAvailable { available } => {
            tracing::info!(
                outcome = outcome.label(),
                available = %available,
                "a newer build is available — offering a reload"
            );
            show_update_banner();
        }
        UpdateCheck::PinnedDeliberately { available } => tracing::info!(
            outcome = outcome.label(),
            available = %available,
            "a newer build is available and this page is a retained shell — not offering it"
        ),
        UpdateCheck::CouldNotCheck(why) => tracing::info!(
            outcome = outcome.label(),
            reason = ?why,
            "could not establish whether a newer build is available"
        ),
        UpdateCheck::Current | UpdateCheck::RunningBuildUnidentifiable => {
            tracing::info!(outcome = outcome.label(), "update check complete")
        }
    }
}

/// The banner. Reload + Dismiss, inline-onclick so there is no Rust `Closure`
/// to own or leak (D12 / AP1) — the `watchdog.rs` / `storage_durability.rs`
/// pattern, and the same id the old `index.html` banner used so the two can
/// never stack during a partial rollout.
#[cfg(target_arch = "wasm32")]
fn show_update_banner() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if doc.get_element_by_id("update-banner").is_some() {
        return; // never stack
    }
    let Ok(banner) = doc.create_element("div") else {
        return;
    };
    banner.set_id("update-banner");
    let _ = banner.set_attribute(
        "style",
        "display:flex;align-items:center;gap:12px;padding:8px 14px;background:#13283a;\
         color:#eee;border-bottom:1px solid #2f6ea3;\
         font:13px/1.4 system-ui,-apple-system,sans-serif;",
    );
    if let Ok(text) = doc.create_element("span") {
        text.set_text_content(Some(&crate::i18n::t("update.available", &[])));
        let _ = text.set_attribute("style", "flex:1;");
        let _ = banner.append_child(&text);
    }
    if let Ok(btn) = doc.create_element("button") {
        btn.set_text_content(Some(&crate::i18n::t("btn.reload", &[])));
        let _ = btn.set_attribute(
            "style",
            "padding:3px 12px;cursor:pointer;background:#2f6ea3;color:#fff;\
             border:1px solid #4a90c2;border-radius:4px;font-size:12px;",
        );
        let _ = btn.set_attribute("onclick", "location.reload()");
        let _ = banner.append_child(&btn);
    }
    if let Ok(btn) = doc.create_element("button") {
        btn.set_text_content(Some(&crate::i18n::t("btn.dismiss", &[])));
        let _ = btn.set_attribute(
            "style",
            "padding:3px 10px;cursor:pointer;background:transparent;color:#eee;\
             border:1px solid currentColor;border-radius:4px;font-size:12px;",
        );
        let _ = btn.set_attribute("onclick", "this.parentNode && this.parentNode.remove()");
        let _ = banner.append_child(&btn);
    }
    if let Some(layout) = doc.get_element_by_id("app-layout") {
        let _ = layout.insert_before(&banner, layout.first_child().as_ref());
    } else if let Some(body) = doc.body() {
        let _ = body.insert_before(&banner, body.first_child().as_ref());
    }
}

#[cfg(target_arch = "wasm32")]
fn document_hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| js_sys::Reflect::get(&d, &"hidden".into()).ok())
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = "aaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbb";

    fn shell(hash: &str) -> OriginRead {
        OriginRead::Served(format!(
            r#"<head><script type="module">import init from '/entity-browser-{hash}.js';</script></head>"#
        ))
    }

    #[test]
    fn the_same_bundle_is_current_and_says_nothing() {
        let c = decide(Some(RUNNING), &shell(RUNNING), true);
        assert_eq!(c, UpdateCheck::Current);
        assert!(!c.prompts());
    }

    #[test]
    fn a_different_bundle_is_the_one_outcome_that_prompts() {
        let c = decide(Some(RUNNING), &shell(OTHER), true);
        assert_eq!(c, UpdateCheck::NewBuildAvailable { available: OTHER.to_string() });
        assert!(c.prompts());
    }

    /// **The production case that misfired, as a test.** An `sw.js`-only deploy
    /// leaves every hashed asset — and therefore the bundle reference in the
    /// shell — untouched. The old trigger fired here and this one must not:
    /// reloading would deliver the same application.
    #[test]
    fn a_deploy_that_changes_only_unhashed_assets_does_not_prompt() {
        // Same bundle hash, different everything else a shell can carry.
        let served = OriginRead::Served(format!(
            r#"<head><meta name="entity-build" content="a-totally-different-commit">
               <script type="module">import init from '/entity-browser-{RUNNING}.js';</script>
               <link rel="preload" href="/entity-browser-{RUNNING}_bg.wasm"></head>"#
        ));
        let c = decide(Some(RUNNING), &served, true);
        assert_eq!(
            c,
            UpdateCheck::Current,
            "a change confined to unhashed assets moved the prompt — this is the sw.js \
             defect rebuilt on a different artifact"
        );
    }

    /// The converse, and the one that made the old banner *silent*: the client
    /// changed completely and `sw.js` did not.
    #[test]
    fn a_deploy_that_changes_the_bundle_prompts_even_with_an_identical_worker() {
        assert!(decide(Some(RUNNING), &shell(OTHER), true).prompts());
    }

    #[test]
    fn a_retained_shell_is_not_nagged_to_leave_the_build_it_was_rolled_back_to() {
        let c = decide(Some(RUNNING), &shell(OTHER), false);
        assert_eq!(c, UpdateCheck::PinnedDeliberately { available: OTHER.to_string() });
        assert!(!c.prompts(), "the banner would be arguing with C10's pin");
    }

    #[test]
    fn only_the_canonical_shell_paths_are_canonical() {
        assert!(is_canonical_shell("/"));
        assert!(is_canonical_shell("/index.html"));
        assert!(!is_canonical_shell("/builds/deadbeefdeadbeef/index.html"));
        assert!(!is_canonical_shell("/builds/"));
    }

    /// **Nothing that established nothing may read as "you are up to date".**
    /// The same invariant the health card's `is_clear` carries, asserted here
    /// as a property of the set rather than as a count, so a sixth outcome
    /// cannot be added quietly on the wrong side of it.
    #[test]
    fn no_outcome_that_established_nothing_is_current_or_prompts() {
        let establishes_nothing = [
            decide(Some(RUNNING), &OriginRead::Unheard, true),
            decide(Some(RUNNING), &OriginRead::NotOk(502), true),
            decide(Some(RUNNING), &OriginRead::NotOk(404), true),
            decide(Some(RUNNING), &OriginRead::Served("<head></head>".into()), true),
            decide(None, &shell(OTHER), true),
        ];
        for c in &establishes_nothing {
            assert_ne!(
                *c,
                UpdateCheck::Current,
                "{} was reported as up to date, which it did not establish",
                c.label()
            );
            assert!(
                !c.prompts(),
                "{} raised a banner off a check that concluded nothing",
                c.label()
            );
        }
    }

    #[test]
    fn the_reasons_a_check_failed_stay_apart() {
        assert_eq!(
            decide(Some(RUNNING), &OriginRead::Unheard, true),
            UpdateCheck::CouldNotCheck(Unknown::Unheard)
        );
        assert_eq!(
            decide(Some(RUNNING), &OriginRead::NotOk(502), true),
            UpdateCheck::CouldNotCheck(Unknown::OriginStatus(502))
        );
        assert_eq!(
            decide(Some(RUNNING), &OriginRead::Served("<head></head>".into()), true),
            UpdateCheck::CouldNotCheck(Unknown::OriginUnidentifiable),
            "an answer we cannot read is not the same as no answer — one points at the \
             build, the other at the link"
        );
    }

    /// A page that cannot name its own build reports *that*, ahead of whatever
    /// the network did — the fact does not change on the next attempt.
    #[test]
    fn a_page_that_cannot_name_its_own_build_says_so_before_it_blames_the_network() {
        assert_eq!(
            decide(None, &OriginRead::Unheard, true),
            UpdateCheck::RunningBuildUnidentifiable
        );
    }

    #[test]
    fn every_outcome_has_its_own_word_and_exactly_one_prompts() {
        let all = [
            UpdateCheck::Current,
            UpdateCheck::NewBuildAvailable { available: OTHER.into() },
            UpdateCheck::PinnedDeliberately { available: OTHER.into() },
            UpdateCheck::CouldNotCheck(Unknown::Unheard),
            UpdateCheck::RunningBuildUnidentifiable,
        ];
        let words: std::collections::BTreeSet<&str> = all.iter().map(|c| c.label()).collect();
        assert_eq!(words.len(), all.len(), "two outcomes share a log word");
        assert_eq!(
            all.iter().filter(|c| c.prompts()).count(),
            1,
            "exactly one outcome may put a banner in front of someone"
        );
        assert_eq!(all.len(), 5, "an outcome was added — decide which side of `prompts` it is on");
    }
}

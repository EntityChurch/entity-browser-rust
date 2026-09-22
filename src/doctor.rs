//! Entity Doctor — *do my beliefs still match the world?*
//!
//! # The quadrant nothing covered
//!
//! | | boots peer? | asks | works when |
//! |---|---|---|---|
//! | **System Recovery** (L1 BIOS) | no — by design | *are the containers there?* | the app is **dead** |
//! | **System Overview** (L4) | yes | *what is the runtime topology?* | the app is **healthy** |
//! | **Entity Doctor** | yes | ***do my beliefs match the world?*** | the app is **alive and wrong** |
//!
//! Both known incidents landed in the third row, and it is the one a local-first
//! system is most prone to: everything runs, nothing errors, and the data is
//! authoritative about a world that moved. `StorageWindow` has the same blind
//! spot one window over — `entity_count`, `path_count`, `sqlite_bytes` are
//! **quantity, never correctness**, so a tree pointing entirely at an abandoned
//! publisher scores identically to a healthy one. Design §6, §7.1.
//!
//! # The one property that matters more than any individual check
//!
//! **"I could not check" must never render as "healthy."** A diagnostic whose
//! unknown state is indistinguishable from its good state is worse than no
//! diagnostic, because it converts *"I do not know"* into *"you are fine"* on
//! the one screen a worried user is reading. So [`Verdict`] has five states,
//! there is no boolean anywhere in this module, and every `match` on an input
//! enumerates its arms rather than falling into a `_ =>` that would hand the
//! weakest evidence the strongest sentence (AP40).
//!
//! # It fixes things too, and that half is the point
//!
//! **A detector with nothing behind it is half a plan, and the half that never
//! gets validated** — it leaves you waiting for a real incident to discover
//! whether the repair even works. So a finding carries an optional
//! [`Remedy`], and the flow around it is built once here rather than per
//! repair: *say what it would change · change it · report what happened*
//! (§7.4). One repair is wired end to end — [`Remedy::RetryFailedRefreshes`],
//! incident B's — so the pattern is exercised rather than described, and a
//! second repair is a variant plus an arm.
//!
//! **The bound is non-destructive, and it is enforced rather than promised.**
//! There is no export path (design §5), so the rule is *no surface may offer a
//! destructive repair it cannot first export around*.
//! [`Remedy::is_non_destructive`] holds that as a predicate and a test asserts
//! it of every variant, so adding a destructive repair fails the build instead
//! of a review.
//!
//! Most findings carry **no** remedy, and that is honest: they report on a
//! world this app does not control, and a button that cannot help is worse than
//! none. Where the repair lives elsewhere, the finding names what does it.
//!
//! # Where this appears
//!
//! **Not in a window of its own.** The design proposed a third surface; the
//! operator's call (2026-09-01) is that it belongs in **System Overview** —
//! *you go to the doctor when there is a problem*. That is also the better
//! answer to §7.3's *"a diagnostic the user must remember does not exist"*: a
//! new window nobody links to would rebuild System Recovery's trap exactly.
//! The section is **quiet when everything agrees** — one line — and opens up
//! only when something does not. No user-facing string says "Doctor"; the
//! module keeps the design's name so the code is traceable to the document that
//! argued for it.
//!
//! # i18n — a stated debt, not an oversight
//!
//! **Every string a person reads lives in this file**, including the section's
//! own chrome ([`copy`]), so translating this surface is one extraction from
//! one module rather than a hunt through a renderer. Today it is English only:
//! the operator's instruction (2026-09-01) was to build it and look at it
//! before committing 30 locales to wording that is still moving.
//!
//! **The debt is made visible rather than left to be discovered.** This module
//! never touches the DOM, so `tools/i18n_prose_scan.py` — whose file set is
//! *"can it reach `set_text` / `components::` / a banner"* — would not have seen
//! it at all, and `make lint` would have stayed green with a whole surface
//! un-extracted. That is the same shape as the gap the scanner itself was
//! written to close (AUDIT-I18N-COVERAGE-GAP-2026-07-19: the metric read 0
//! while ~440 strings sat un-extracted). So this file is **added to the
//! scanner's set** and its count is recorded in the baseline: the debt is a
//! number in a checked-in file that only ratchets down.
//!
//! # Why the checks are pure functions
//!
//! Every check here takes its inputs as arguments and returns a [`Finding`]. No
//! fetching, no store access, no DOM. That is what lets the verdict logic —
//! which is the whole product — be gated by `make test` on every arm, instead
//! of only through a browser. The impure half (reading the durable config,
//! reading the deployment document) lives in the window.

use crate::deployment_config::DocumentRead;
use crate::refresh_ledger::{RefreshOutcome, Snapshot};

/// Which question a finding answers. The numbering is the design's (§7.2), kept
/// so a finding can be traced back to the incident that earned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// §7.2 #1 — persisted `home_site.peer` vs `/entity-deployment.json`.
    /// **Catches incident A outright.**
    DomainIdentity,
    /// §7.2 #2 — the origin answers, but everything under one peer is withheld.
    FetchFailureByPeer,
    /// §7.2 #3 — did every set this session tried to load actually load?
    /// **Catches incident B**, which no check in arch's original list covers.
    CatalogCompleteness,
}

impl Check {
    /// The heading a person reads. Not the enum name.
    pub fn title(&self) -> &'static str {
        match self {
            Check::DomainIdentity => "Who publishes this site",
            Check::FetchFailureByPeer => "Whether a publisher is still answering",
            Check::CatalogCompleteness => "Whether everything finished loading",
        }
    }

    /// Stable identifier for logs and gates — never the display string, which
    /// is free to be reworded.
    pub fn key(&self) -> &'static str {
        match self {
            Check::DomainIdentity => "domain-identity",
            Check::FetchFailureByPeer => "fetch-failure-by-peer",
            Check::CatalogCompleteness => "catalog-completeness",
        }
    }
}

/// What a check concluded. **Five states, and four of them are not "healthy".**
///
/// The ordering is severity, worst first, so a surface can sort by it and a
/// summary can take the maximum without a second table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// The belief and the source disagree. This is the finding.
    Diverges,
    /// The source could not be consulted, so **nothing is known** — not
    /// "nothing is wrong". Distinct from [`SourceSilent`](Verdict::SourceSilent)
    /// exactly the way `Unheard` is distinct from `NoDocument`: one is nobody
    /// answered, the other is somebody answered "I have none".
    Undetermined,
    /// The source answered and declares nothing to compare against. A
    /// deployment that ships no document on purpose is not a fault, and must not
    /// be reported as one.
    SourceSilent,
    /// There is no belief to check yet.
    NothingToCheck,
    /// Checked, and they agree.
    Agrees,
}

impl Verdict {
    /// One word, for the status chip and for the log line. Five distinct
    /// labels; a sixth verdict cannot silently reuse one (gated).
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Diverges => "diverges",
            Verdict::Undetermined => "undetermined",
            Verdict::SourceSilent => "source-silent",
            Verdict::NothingToCheck => "nothing-to-check",
            Verdict::Agrees => "agrees",
        }
    }

    /// Whether this verdict is a positive statement of health. **Only one is.**
    /// The method exists so no call site has to re-derive that, and so the
    /// question "does this count as OK?" has exactly one answer in the tree.
    pub fn is_clear(&self) -> bool {
        matches!(self, Verdict::Agrees)
    }

    /// **Is this worth putting in front of a person?** — and it is deliberately
    /// *not* `!is_clear()`.
    ///
    /// That was the first implementation and seeing it running is what killed
    /// it: on an ordinary healthy profile all three checks come back
    /// `SourceSilent` or `NothingToCheck` — nothing is wrong, there is simply
    /// nothing to compare yet — and the section rendered three paragraphs of
    /// explanation under a heading that says **Problems**. A screen that
    /// reports non-problems as problems is one people learn to close, which
    /// costs exactly the attention the real finding will need.
    ///
    /// So: a divergence is reported, and a check that *tried and could not
    /// tell* is reported (a user who is offline is owed that, and it is the
    /// state that must never masquerade as health). A check with nothing to
    /// compare against, or nothing yet asked of it, is **not a problem** and
    /// stays out of the list — it is summarised in the quiet line instead.
    ///
    /// *If there are no problems you do not go to the doctor* — the operator's
    /// framing, and this predicate is where it is enforced.
    pub fn warrants_attention(&self) -> bool {
        matches!(self, Verdict::Diverges | Verdict::Undetermined)
    }

    /// The chip a person reads. Separate from [`label`](Self::label), which is
    /// the stable log/gate token and must stay a token — a reworded chip that
    /// silently changed a log field is how a grep stops finding an incident.
    ///
    /// Read these four non-clear words together: none of them says *"OK"*, and
    /// none says *"error"* either. Three of the five mean *this was not
    /// established*, and the surface has to carry that difference rather than
    /// rounding it to a tick or a cross.
    pub fn chip(&self) -> &'static str {
        match self {
            Verdict::Diverges => "needs attention",
            Verdict::Undetermined => "could not check",
            Verdict::SourceSilent => "nothing to compare",
            Verdict::NothingToCheck => "not checked yet",
            Verdict::Agrees => "all good",
        }
    }

    /// How much of the user's attention this deserves. **Three bands from five
    /// verdicts** — and the mapping is where the "unknown is not good news"
    /// rule becomes visible: the three verdicts that establish nothing all land
    /// on `Unknown`, never on `Clear`. Only `Agrees` is `Clear`.
    #[cfg(target_arch = "wasm32")]
    pub fn tone(&self) -> crate::dom::components::HealthTone {
        use crate::dom::components::HealthTone;
        match self {
            Verdict::Diverges => HealthTone::Attention,
            Verdict::Undetermined | Verdict::SourceSilent | Verdict::NothingToCheck => {
                HealthTone::Unknown
            }
            Verdict::Agrees => HealthTone::Clear,
        }
    }
}

/// The section's own copy. Kept here with everything else a person reads, so
/// there is **one file** to extract when this is translated — see the module
/// note on i18n.
pub mod copy {
    /// The heading. Not "Doctor" — the operator's call, and the right one: this
    /// is the place you look when something is wrong, not a branded feature.
    pub const TITLE: &str = "Problems";
    /// The quiet state. *If there are no problems you do not go to the doctor* —
    /// so a healthy system gets one line, not a dashboard of green ticks.
    pub const ALL_CLEAR: &str = "No problems found.";
    /// …but a diagnostic that renders nothing when healthy cannot be told from
    /// one that never ran, so the quiet line still says when it last looked.
    /// The quiet line's second half. It names how many checks ran and how many
    /// had nothing to compare against, because *"no problems"* on its own is
    /// indistinguishable from *"nothing ran"* — and a check that could not find
    /// a source has not cleared anything.
    pub const CHECKED_FMT: &str = "{n} checks ran just now.";
    pub const CHECKED_WITH_GAPS_FMT: &str =
        "{n} checks ran just now; {q} had nothing to compare against yet.";
    pub const CHECKING: &str = "Checking…";
    pub const RECHECK: &str = "Check again";
    /// Labels for the two halves of every finding — the design's *belief ·
    /// source* pair, which is what makes a finding auditable instead of an
    /// opinion.
    pub const BELIEF: &str = "This machine:";
    pub const SOURCE: &str = "Checked against:";
    /// Shown when at least one check could not be run. It is deliberately not
    /// reassuring.
    pub const SOME_UNDETERMINED: &str =
        "Some checks could not be completed, so this is not a clean bill of health.";
}

/// A repair a finding offers. **This is the pattern, not a special case** —
/// §7.5's remedy ledger in miniature: *a known defect with a known repair,
/// each with a detection predicate and a remedy*. A new repair is a variant
/// plus an arm, and it inherits the whole flow below for free.
///
/// The shape is §7.4's, in order: **say what it would change, change it, then
/// report what happened.** [`effect`](Remedy::effect) is rendered next to the
/// button, before anything runs — a diagnostic that acts on a stuck user's
/// machine without first saying what it will touch is asking for trust it has
/// not earned.
///
/// **Every remedy here is non-destructive, and that is a hard bound, not a
/// coincidence of the first one:** there is no export path (design §5), and the
/// rule that falls out is *no surface may offer a destructive repair it cannot
/// first export around*. A remedy that deletes, overwrites or resets anything
/// the user authored does not belong in this enum until that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Drop the once-per-window fetch guard so the sets that failed are tried
    /// again. Incident B's repair, which until now was only reachable by
    /// closing and re-opening the window.
    RetryFailedRefreshes,
}

impl Remedy {
    /// The button.
    pub fn label(&self) -> &'static str {
        match self {
            Remedy::RetryFailedRefreshes => "Try loading them again",
        }
    }

    /// What it would change, shown **before** it is pressed.
    pub fn effect(&self) -> &'static str {
        match self {
            Remedy::RetryFailedRefreshes => {
                "Asks any open Apps window to fetch the sets that failed, once more. \
                 Nothing is deleted, overwritten or reset, and anything already loaded \
                 stays as it is."
            }
        }
    }

    /// Stable identifier for the action wire and for gates.
    pub fn key(&self) -> &'static str {
        match self {
            Remedy::RetryFailedRefreshes => "retry-failed-refreshes",
        }
    }

    /// Resolve a wire value back to a remedy. Unknown values yield `None` — a
    /// surface must never guess which repair it was asked for.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "retry-failed-refreshes" => Some(Remedy::RetryFailedRefreshes),
            _ => None,
        }
    }

    /// **Nothing here may destroy user data.** Held as an explicit predicate
    /// rather than a comment so the rule has an enforcement point: the gate
    /// below asserts it of every variant, so adding a destructive repair fails
    /// the build instead of a review.
    pub fn is_non_destructive(&self) -> bool {
        match self {
            Remedy::RetryFailedRefreshes => true,
        }
    }

    /// Run it. Returns what actually happened, for the surface to report —
    /// never a bare success, and never silence.
    pub fn apply(&self) -> RemedyOutcome {
        match self {
            Remedy::RetryFailedRefreshes => {
                let generation = crate::refresh_ledger::request_retry();
                tracing::info!(
                    remedy = self.key(),
                    generation,
                    "health: remedy applied — asked open launchers to retry the sets that \
                     failed"
                );
                RemedyOutcome::Requested
            }
        }
    }
}

/// What applying a remedy did. **Deliberately not a bool.** The retry is a
/// request to a mechanism that answers later, and reporting that as *"fixed"*
/// would be the same lie this whole surface exists to stop telling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemedyOutcome {
    /// Asked. The result arrives when the mechanism answers, and the check
    /// above re-reports it from the same evidence as before.
    Requested,
}

impl RemedyOutcome {
    /// What to tell the user. Note what it does NOT say: not "fixed".
    pub fn message(&self) -> &'static str {
        match self {
            RemedyOutcome::Requested => {
                "Asked. This section updates on its own when the retry finishes — if the \
                 publisher still does not have them, it will say so again."
            }
        }
    }
}

/// One check's result, in the design's shape: *(belief · source · verdict ·
/// remedy)*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: Check,
    pub verdict: Verdict,
    /// What this profile believes — the value it is carrying around.
    pub belief: String,
    /// Where that belief would be re-checked against, and what it said.
    pub source: String,
    /// What it means and what to do. Never a promise this surface cannot keep.
    pub detail: String,
    /// The repair, when there is one. `None` is the common case and is
    /// honest — most findings here are reports about a world this app does not
    /// control, and offering a button that cannot help is worse than none.
    pub remedy: Option<Remedy>,
}

/// **Check 1 — domain identity.** Does the publisher this profile is pointed at
/// match the one the domain currently declares?
///
/// This is incident A: `ecdeos.org` re-keyed, every returning visitor kept
/// asking the retired publisher for anything new, local content kept rendering,
/// and nothing looked broken. Boot reconciles this now
/// (`peer_supersession`); the check exists because *reconciliation that
/// happened silently is indistinguishable from reconciliation that did not*,
/// and because the reconcile cannot run when the document could not be read.
///
/// `believed` is `None` when this profile has no home publisher recorded.
pub fn check_domain_identity(believed: Option<&str>, doc: &DocumentRead) -> Finding {
    let declared: Option<&str> = match doc {
        DocumentRead::Served(cfg) => cfg
            .home_site
            .as_ref()
            .map(|h| h.peer_id.as_str())
            .filter(|p| !p.is_empty()),
        // Every other arm is enumerated below rather than folded in here: what
        // the domain declares is only meaningful when it answered.
        _ => None,
    };

    let belief = match believed {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => "no publisher recorded".to_string(),
    };

    // The arms are written out. A `_ =>` here would be the AP40 mistake in its
    // purest form — the residue of four different failure shapes inheriting
    // whichever sentence happened to be last.
    let (verdict, source, detail) = match (believed.filter(|p| !p.is_empty()), doc, declared) {
        (_, DocumentRead::Unheard, _) => (
            Verdict::Undetermined,
            "this domain did not answer".to_string(),
            "Nothing was heard from the domain, so nothing could be compared and nothing is \
             known — this is not a clean bill of health. Everything already on this machine \
             is unaffected and still accurate. Try again when you are back online."
                .to_string(),
        ),
        // **`OriginError` and `Unreadable` are Undetermined, not silent.** A 502
        // or a truncated document says nothing at all about who publishes this
        // domain; the first version of the enum one layer down collapsed every
        // non-2xx into "this deployment serves no config by choice", which is a
        // false claim about the deployer's intent (AP40). Repeating that here
        // would put the same false claim in front of a user.
        (_, DocumentRead::OriginError { status }, _) => (
            Verdict::Undetermined,
            format!("this domain answered with an error (HTTP {status})"),
            "The domain is reachable but returned a fault instead of its configuration, so \
             nothing could be compared. That is a problem at the domain, not on this \
             machine, and it says nothing about whether your publisher is current."
                .to_string(),
        ),
        (_, DocumentRead::Unreadable { status }, _) => (
            Verdict::Undetermined,
            format!("this domain's configuration could not be read (HTTP {status})"),
            "The domain answered with something this app could not parse — a truncated or \
             half-written file, or an error page served as a success. Nothing could be \
             compared, and nothing here is a statement about your machine."
                .to_string(),
        ),
        (_, DocumentRead::NoDocument { .. }, _) => (
            Verdict::SourceSilent,
            "this domain serves no deployment document".to_string(),
            "The domain answered and publishes no configuration, which is a legitimate \
             choice and not a fault. There is nothing here to check against."
                .to_string(),
        ),
        (_, DocumentRead::Served(_), None) => (
            Verdict::SourceSilent,
            "this domain's document names no publisher".to_string(),
            "The domain answered, and its configuration does not say who publishes it. \
             There is nothing to compare against."
                .to_string(),
        ),
        (None, DocumentRead::Served(_), Some(d)) => (
            Verdict::NothingToCheck,
            format!("this domain publishes as {d}"),
            "This profile has not recorded a publisher of its own yet, so there is no \
             belief to check. It will adopt the domain's on the next boot."
                .to_string(),
        ),
        (Some(b), DocumentRead::Served(_), Some(d)) if b == d => (
            Verdict::Agrees,
            format!("this domain publishes as {d}"),
            "This profile is pointed at the publisher the domain currently declares. \
             Routing is not your problem."
                .to_string(),
        ),
        (Some(_), DocumentRead::Served(_), Some(d)) => (
            Verdict::Diverges,
            format!("this domain publishes as {d}"),
            "This profile is pointed at a publisher this domain no longer uses. Everything \
             already in your tree keeps rendering and nothing looks broken, but every \
             request for anything new goes to the old publisher and comes back empty. \
             Opening the app again repairs this on the next load; your content is not \
             affected and must not be cleared."
                .to_string(),
        ),
    };

    Finding { check: Check::DomainIdentity, verdict, belief, source, detail, remedy: None }
}

/// **Check 2 — fetch failure aggregated by peer.**
///
/// The signature from incident A, stated as a rule: *the origin is answering,
/// and yet every single thing asked of one peer comes back "not here"*. That is
/// evidence about the peer, not about the network — and it is the join nobody
/// made at the time, when three separate signals all existed at the moment of
/// failure.
///
/// **A peer with any successful refresh is not reported**, however many
/// withholdings sit beside it: a partly-served publisher is a content gap
/// (check 3's subject), not a wrong identity. And **`Unreachable` is never
/// evidence here** — a dropped connection says nothing about who the publisher
/// is, and treating it as if it did would blame a re-key for a tunnel.
pub fn check_fetch_failure_by_peer(ledger: &Snapshot) -> Finding {
    use std::collections::BTreeMap;

    // (any_current, withheld_count) per peer.
    let mut per_peer: BTreeMap<&str, (bool, usize)> = BTreeMap::new();
    for r in &ledger.records {
        let e = per_peer.entry(r.peer_id.as_str()).or_insert((false, 0));
        match r.outcome {
            RefreshOutcome::Current => e.0 = true,
            RefreshOutcome::Withheld => e.1 += 1,
            RefreshOutcome::Unreachable(_) => {}
        }
    }

    let suspect: Vec<(&str, usize)> = per_peer
        .iter()
        .filter(|(_, (any_current, withheld))| !*any_current && *withheld > 0)
        .map(|(p, (_, w))| (*p, *w))
        .collect();

    if !suspect.is_empty() {
        let named = suspect
            .iter()
            .map(|(p, w)| format!("{} ({w} request(s) refused)", short(p)))
            .collect::<Vec<_>>()
            .join(", ");
        return Finding {
            check: Check::FetchFailureByPeer,
            verdict: Verdict::Diverges,
            belief: format!("this profile is asking {named}"),
            source: "the publisher answered, and said it has none of it".to_string(),
            detail: "The publisher is reachable and is refusing every single thing this \
                     profile asks it for. That is the signature of a publisher that moved: \
                     the address still resolves, and nothing is behind it any more. Check \
                     \"Who publishes this site\" above — if that says the domain now names \
                     someone else, this is the same fault seen from the other end."
                .to_string(),
            remedy: None,
        };
    }

    // Nothing suspect. Now say WHY, without ever letting "we asked nothing"
    // wear the same words as "we asked and it was fine".
    if ledger.attempted == 0 {
        return Finding {
            check: Check::FetchFailureByPeer,
            verdict: Verdict::NothingToCheck,
            belief: "nothing has been fetched from a publisher yet".to_string(),
            source: "no request has been made this session".to_string(),
            detail: "This is recorded from the moment the app starts and is cleared by a \
                     reload, so an empty list means nothing has been asked for yet — not \
                     that everything succeeded. Open the Apps window and come back."
                .to_string(),
            remedy: None,
        };
    }
    Finding {
        check: Check::FetchFailureByPeer,
        verdict: Verdict::Agrees,
        belief: format!("{} request(s) made this session", ledger.attempted),
        source: "every publisher asked has served something".to_string(),
        detail: "No publisher refused everything it was asked for, so nothing here points \
                 at a publisher that has moved."
            .to_string(),
        remedy: None,
    }
}

/// **Check 3 — catalog / set completeness.** *Did everything I tried to load
/// actually load?*
///
/// Incident B, directly. A set whose fetch ladder ran out writes nothing, fires
/// no subscription, and leaves the grid rendering the other set — complete to
/// look at, and missing half its contents. The only report was a `warn!`.
///
/// **Both non-current outcomes are reported, and they are reported
/// differently**, because the advice differs: a withheld set is the publisher's
/// doing and will not fix itself by waiting, and an unreachable one usually
/// will.
pub fn check_catalog_completeness(ledger: &Snapshot) -> Finding {
    let failed: Vec<&crate::refresh_ledger::RefreshRecord> =
        ledger.records.iter().filter(|r| !r.outcome.is_current()).collect();

    if failed.is_empty() {
        if ledger.attempted == 0 {
            return Finding {
                check: Check::CatalogCompleteness,
                verdict: Verdict::NothingToCheck,
                belief: "nothing has been loaded from a publisher yet".to_string(),
                source: "no set has been asked for this session".to_string(),
                detail: "Nothing has tried to load yet, so there is nothing to be missing. \
                         This is not the same as everything having loaded."
                    .to_string(),
                remedy: None,
            };
        }
        return Finding {
            check: Check::CatalogCompleteness,
            verdict: Verdict::Agrees,
            belief: format!("{} set(s) loaded", ledger.records.len()),
            source: format!("{} attempt(s) this session", ledger.attempted),
            detail: if ledger.truncated {
                "Everything recorded loaded — but this session made more requests than the \
                 list can hold, so some are not represented here."
                    .to_string()
            } else {
                "Everything this session asked a publisher for arrived. Nothing is missing \
                 from what you are looking at."
                    .to_string()
            },
            remedy: None,
        };
    }

    let withheld = failed.iter().filter(|r| r.outcome == RefreshOutcome::Withheld).count();
    let named = failed
        .iter()
        .map(|r| format!("{} (from {})", r.what, short(&r.peer_id)))
        .collect::<Vec<_>>()
        .join(", ");

    Finding {
        check: Check::CatalogCompleteness,
        verdict: Verdict::Diverges,
        belief: format!("{} of {} set(s) did not load: {named}", failed.len(), ledger.records.len()),
        source: if withheld == failed.len() {
            "the publisher answered and does not have them".to_string()
        } else if withheld == 0 {
            "the publisher could not be reached".to_string()
        } else {
            "some were refused, some could not be reached".to_string()
        },
        detail: "What you are looking at is incomplete, and it does not say so on its own \
                 — a set that fails to load leaves the others rendering, so the screen \
                 looks finished. Nothing of yours was lost. If they were refused rather \
                 than unreachable, retrying will not help and the publisher no longer has \
                 them."
            .to_string(),
        // **The one finding here that this app can actually act on.** The rest
        // report on a world it does not control; this one is a guard it set
        // itself, so it can drop it.
        remedy: Some(Remedy::RetryFailedRefreshes),
    }
}

/// Shorten a peer id for display the same way the rest of the product does.
fn short(peer_id: &str) -> String {
    if peer_id.len() > 18 {
        format!("{}…{}", &peer_id[..8], &peer_id[peer_id.len() - 6..])
    } else {
        peer_id.to_string()
    }
}

/// The worst verdict across a set of findings — what the window's summary line
/// says. Empty is [`Verdict::NothingToCheck`], never `Agrees`: a run that
/// produced no findings has not established anything.
pub fn overall(findings: &[Finding]) -> Verdict {
    findings.iter().map(|f| f.verdict).min().unwrap_or(Verdict::NothingToCheck)
}

/// Run every check and report. **The one entry point** — a surface calls this
/// and renders what comes back; it does not get to choose a subset, so a new
/// check appears everywhere the moment it is added here.
///
/// The D13 channel is one line per run naming each check and its verdict
/// (`check=… verdict=…`), at `info`. Without it the only record of a check
/// having run is whatever the user happened to be looking at, which is the
/// silence this whole surface exists to end — and it means an incident can be
/// reconstructed from a log the user can hand over.
pub fn run_checks(
    believed_home_peer: Option<&str>,
    doc: &DocumentRead,
    ledger: &Snapshot,
) -> Vec<Finding> {
    let findings = vec![
        check_domain_identity(believed_home_peer, doc),
        check_fetch_failure_by_peer(ledger),
        check_catalog_completeness(ledger),
    ];
    for f in &findings {
        tracing::info!(
            check = f.check.key(),
            verdict = f.verdict.label(),
            belief = %f.belief,
            source = %f.source,
            "health check"
        );
    }
    tracing::info!(
        overall = overall(&findings).label(),
        checks = findings.len(),
        "health checks complete"
    );
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_config::{DeploymentConfig, DocumentRead};
    use crate::refresh_ledger::{RefreshOutcome, RefreshRecord, Snapshot};

    fn served(home_peer: Option<&str>) -> DocumentRead {
        let json = match home_peer {
            Some(p) => format!(r#"{{"home_site":{{"peer":"{p}","site":"demo"}}}}"#),
            None => "{}".to_string(),
        };
        DocumentRead::Served(DeploymentConfig::parse(&json).expect("fixture document parses"))
    }

    fn ledger(records: Vec<RefreshRecord>, attempted: usize) -> Snapshot {
        Snapshot { records, attempted, truncated: false }
    }

    fn rec(peer: &str, what: &str, outcome: RefreshOutcome) -> RefreshRecord {
        RefreshRecord { peer_id: peer.into(), what: what.into(), outcome }
    }

    // ── Check 1 ──────────────────────────────────────────────────────────────

    #[test]
    fn a_profile_pointed_at_a_retired_publisher_diverges() {
        let f = check_domain_identity(Some("peerOLD"), &served(Some("peerNEW")));
        assert_eq!(f.verdict, Verdict::Diverges);
        assert!(f.source.contains("peerNEW"), "the finding must name what the domain says");
        assert!(f.belief.contains("peerOLD"), "and what this profile believes");
    }

    #[test]
    fn a_profile_pointed_at_the_current_publisher_agrees() {
        let f = check_domain_identity(Some("peerA"), &served(Some("peerA")));
        assert_eq!(f.verdict, Verdict::Agrees);
    }

    /// **The property this whole module exists for.** An unreachable domain
    /// must not be able to produce a clear verdict — the failure mode of every
    /// diagnostic is defaulting to green when it learned nothing.
    #[test]
    fn an_unreachable_domain_is_undetermined_and_never_clear() {
        for doc in [
            DocumentRead::Unheard,
            DocumentRead::OriginError { status: 502 },
            DocumentRead::Unreadable { status: 200 },
        ] {
            let f = check_domain_identity(Some("peerA"), &doc);
            assert_eq!(f.verdict, Verdict::Undetermined, "{doc:?}");
            assert!(
                !f.verdict.is_clear(),
                "a source that could not be consulted reported as clear: {doc:?} — this is \
                 the \"I could not check\" → \"you are healthy\" collapse"
            );
        }
    }

    /// `NoDocument` and `Unheard` must not merge. One is the deployment saying
    /// "I publish no configuration", the other is nobody answering — and they
    /// decide whether silence is expected.
    #[test]
    fn a_domain_that_serves_no_document_is_not_the_same_as_one_that_did_not_answer() {
        let silent = check_domain_identity(Some("peerA"), &DocumentRead::NoDocument { status: 404 });
        let unheard = check_domain_identity(Some("peerA"), &DocumentRead::Unheard);
        assert_eq!(silent.verdict, Verdict::SourceSilent);
        assert_eq!(unheard.verdict, Verdict::Undetermined);
        assert_ne!(
            silent.verdict, unheard.verdict,
            "these two arrive by different routes and mean different things"
        );
    }

    /// **A 502 is not a deployment declaring it has no configuration.** The
    /// enum one layer down was fixed for exactly this (AP40 — every non-2xx
    /// became *"serves no config by choice"*, a false claim about the deployer's
    /// intent). This asserts the repaired distinction survives the trip to the
    /// user, because a check that re-collapses it puts the false claim back on
    /// screen with none of the code to blame.
    #[test]
    fn an_origin_fault_is_not_reported_as_a_deployment_that_declares_nothing() {
        let fault = check_domain_identity(Some("peerA"), &DocumentRead::OriginError { status: 502 });
        let chosen = check_domain_identity(Some("peerA"), &DocumentRead::NoDocument { status: 404 });
        assert_eq!(fault.verdict, Verdict::Undetermined);
        assert_eq!(chosen.verdict, Verdict::SourceSilent);
        assert!(
            !fault.detail.contains("legitimate choice"),
            "a server fault was described as the deployer's intent: {fault:?}"
        );
    }

    #[test]
    fn a_served_document_naming_no_publisher_is_silent_not_divergent() {
        let f = check_domain_identity(Some("peerA"), &served(None));
        assert_eq!(f.verdict, Verdict::SourceSilent);
    }

    #[test]
    fn a_profile_with_no_recorded_publisher_has_nothing_to_check() {
        let f = check_domain_identity(None, &served(Some("peerA")));
        assert_eq!(f.verdict, Verdict::NothingToCheck);
        let f = check_domain_identity(Some(""), &served(Some("peerA")));
        assert_eq!(f.verdict, Verdict::NothingToCheck, "an empty id is not a belief");
    }

    // ── Check 2 ──────────────────────────────────────────────────────────────

    #[test]
    fn a_publisher_that_refuses_everything_is_reported() {
        let s = ledger(
            vec![
                rec("peerOLD", "apps", RefreshOutcome::Withheld),
                rec("peerOLD", "games", RefreshOutcome::Withheld),
            ],
            2,
        );
        let f = check_fetch_failure_by_peer(&s);
        assert_eq!(f.verdict, Verdict::Diverges);
        assert!(f.belief.contains("peerOLD"));
    }

    /// A publisher serving *anything* is not a wrong publisher. Reporting it as
    /// one would send a user chasing a re-key for what is a content gap.
    #[test]
    fn a_publisher_that_serves_something_is_not_suspect_however_much_it_withholds() {
        let s = ledger(
            vec![
                rec("peerA", "apps", RefreshOutcome::Current),
                rec("peerA", "games", RefreshOutcome::Withheld),
                rec("peerA", "books", RefreshOutcome::Withheld),
            ],
            3,
        );
        assert_eq!(check_fetch_failure_by_peer(&s).verdict, Verdict::Agrees);
    }

    /// An unreachable network is not evidence about who the publisher is.
    #[test]
    fn unreachable_is_never_evidence_that_a_publisher_moved() {
        let s = ledger(
            vec![
                rec("peerA", "apps", RefreshOutcome::Unreachable("timeout".into())),
                rec("peerA", "games", RefreshOutcome::Unreachable("timeout".into())),
            ],
            2,
        );
        let f = check_fetch_failure_by_peer(&s);
        assert_ne!(
            f.verdict,
            Verdict::Diverges,
            "a dropped connection was read as a moved publisher — that blames a re-key \
             for a tunnel"
        );
    }

    #[test]
    fn an_empty_session_has_nothing_to_check_rather_than_a_clean_bill() {
        let f = check_fetch_failure_by_peer(&ledger(vec![], 0));
        assert_eq!(f.verdict, Verdict::NothingToCheck);
        assert!(!f.verdict.is_clear());
    }

    // ── Check 3 ──────────────────────────────────────────────────────────────

    /// Incident B, in one assertion: one set loaded, one did not, and the
    /// screen would look complete.
    #[test]
    fn a_set_that_failed_to_load_is_reported_even_though_the_others_rendered() {
        let s = ledger(
            vec![
                rec("peerA", "apps", RefreshOutcome::Current),
                rec("peerA", "games", RefreshOutcome::Unreachable("gave up after 5".into())),
            ],
            2,
        );
        let f = check_catalog_completeness(&s);
        assert_eq!(f.verdict, Verdict::Diverges);
        assert!(f.belief.contains("games"), "the finding must name the set: {f:?}");
    }

    #[test]
    fn the_advice_distinguishes_refused_from_unreachable() {
        let refused = check_catalog_completeness(&ledger(
            vec![rec("peerA", "games", RefreshOutcome::Withheld)],
            1,
        ));
        let unreachable = check_catalog_completeness(&ledger(
            vec![rec("peerA", "games", RefreshOutcome::Unreachable("timeout".into()))],
            1,
        ));
        assert_ne!(
            refused.source, unreachable.source,
            "waiting fixes one of these and not the other, so they cannot read the same"
        );
    }

    #[test]
    fn everything_loading_agrees_and_nothing_loading_does_not() {
        let all_good =
            check_catalog_completeness(&ledger(vec![rec("p", "apps", RefreshOutcome::Current)], 1));
        assert_eq!(all_good.verdict, Verdict::Agrees);
        let nothing = check_catalog_completeness(&ledger(vec![], 0));
        assert_eq!(nothing.verdict, Verdict::NothingToCheck);
        assert!(!nothing.verdict.is_clear());
    }

    #[test]
    fn a_truncated_ledger_says_so_rather_than_claiming_everything_loaded() {
        let s = Snapshot {
            records: vec![rec("p", "apps", RefreshOutcome::Current)],
            attempted: 900,
            truncated: true,
        };
        let f = check_catalog_completeness(&s);
        assert_eq!(f.verdict, Verdict::Agrees);
        assert!(
            f.detail.contains("not represented"),
            "a capped ledger reporting an unqualified all-clear is reporting the cap: {f:?}"
        );
    }

    // ── The surface-level invariants ─────────────────────────────────────────

    /// Five verdicts, five distinct words, and the COUNT is asserted — so a
    /// sixth state cannot quietly reuse an existing label. (The same shape as
    /// `every_hydration_outcome_has_its_own_word`, which caught exactly this.)
    #[test]
    fn every_verdict_has_its_own_word() {
        let all = [
            Verdict::Diverges,
            Verdict::Undetermined,
            Verdict::SourceSilent,
            Verdict::NothingToCheck,
            Verdict::Agrees,
        ];
        let labels: std::collections::BTreeSet<&str> = all.iter().map(|v| v.label()).collect();
        assert_eq!(labels.len(), all.len(), "two verdicts share a word");
        assert_eq!(all.len(), 5, "a verdict was added or removed without updating this test");
    }

    /// **Exactly one verdict may read as OK.** If a second ever does, every
    /// summary line and status chip in the product silently widens what it
    /// calls healthy.
    #[test]
    fn only_agrees_counts_as_clear() {
        let clear = [
            Verdict::Diverges,
            Verdict::Undetermined,
            Verdict::SourceSilent,
            Verdict::NothingToCheck,
            Verdict::Agrees,
        ]
        .iter()
        .filter(|v| v.is_clear())
        .count();
        assert_eq!(clear, 1, "more than one verdict reports as a clean bill of health");
    }

    /// The summary must be the worst thing found, and an empty run must not
    /// summarise as healthy.
    #[test]
    fn the_summary_takes_the_worst_finding_and_an_empty_run_is_not_healthy() {
        let f = |v: Verdict| Finding {
            check: Check::DomainIdentity,
            verdict: v,
            belief: String::new(),
            source: String::new(),
            detail: String::new(),
            remedy: None,
        };
        assert_eq!(overall(&[f(Verdict::Agrees), f(Verdict::Diverges)]), Verdict::Diverges);
        assert_eq!(overall(&[f(Verdict::Agrees), f(Verdict::Undetermined)]), Verdict::Undetermined);
        assert_eq!(overall(&[f(Verdict::Agrees)]), Verdict::Agrees);
        assert_eq!(overall(&[]), Verdict::NothingToCheck);
        assert!(!overall(&[]).is_clear());
    }

    // ── The remedy half ──────────────────────────────────────────────────────

    /// **The bound, as a gate rather than a promise.** There is no export path
    /// (design §5), so no repair offered here may destroy anything. If a future
    /// remedy needs to, this test is what stops it landing quietly — and the
    /// answer is to build export first, not to edit this line.
    #[test]
    fn no_remedy_is_destructive() {
        for r in ALL_REMEDIES {
            assert!(
                r.is_non_destructive(),
                "{} can destroy user data, and there is still no export path to undo it \
                 with. Build the export (design §5) before offering this repair.",
                r.key()
            );
        }
    }

    /// A remedy's wire key must round-trip, and an unknown one must resolve to
    /// `None` rather than to the nearest match. A surface that guesses which
    /// repair it was asked to run is a surface that runs the wrong one.
    #[test]
    fn a_remedy_round_trips_by_key_and_an_unknown_key_resolves_to_nothing() {
        for r in ALL_REMEDIES {
            assert_eq!(Remedy::from_key(r.key()), Some(*r), "{} did not round-trip", r.key());
        }
        assert_eq!(Remedy::from_key("retry"), None, "a prefix must not resolve");
        assert_eq!(Remedy::from_key(""), None);
        assert_eq!(Remedy::from_key("drop-everything"), None);
    }

    /// Every remedy has its own label, its own effect sentence and its own key,
    /// and the count is asserted — so a second repair cannot silently inherit
    /// the first one's "what this will change" text, which is the one string
    /// standing between a user and an action they did not consent to.
    #[test]
    fn every_remedy_states_its_own_effect() {
        use std::collections::BTreeSet;
        let keys: BTreeSet<&str> = ALL_REMEDIES.iter().map(|r| r.key()).collect();
        let labels: BTreeSet<&str> = ALL_REMEDIES.iter().map(|r| r.label()).collect();
        let effects: BTreeSet<&str> = ALL_REMEDIES.iter().map(|r| r.effect()).collect();
        assert_eq!(keys.len(), ALL_REMEDIES.len());
        assert_eq!(labels.len(), ALL_REMEDIES.len());
        assert_eq!(effects.len(), ALL_REMEDIES.len(), "two remedies share an effect sentence");
        assert_eq!(ALL_REMEDIES.len(), 1, "a remedy was added; update this count deliberately");
    }

    /// Applying the retry must actually move the signal the mechanism watches —
    /// otherwise the button is a placebo that reports success. And the outcome
    /// must say *requested*, not *fixed*: the retry is a request to something
    /// that answers later.
    #[test]
    fn the_retry_remedy_moves_the_signal_and_reports_only_what_it_did() {
        crate::refresh_ledger::reset_for_test();
        let before = crate::refresh_ledger::retry_generation();
        let outcome = Remedy::RetryFailedRefreshes.apply();
        assert_eq!(outcome, RemedyOutcome::Requested);
        assert!(
            crate::refresh_ledger::retry_generation() > before,
            "the remedy reported success without moving the signal any launcher watches — \
             that is a placebo button on a diagnostic screen"
        );
    }

    /// The finding that has a repair must offer it, and the ones that do not
    /// must not pretend. A "Retry" button under *"this domain re-keyed"* would
    /// do nothing and teach the user the buttons here are decoration.
    #[test]
    fn only_the_finding_this_app_can_act_on_carries_a_remedy() {
        let incomplete = check_catalog_completeness(&ledger(
            vec![rec("p", "games", RefreshOutcome::Unreachable("gave up".into()))],
            1,
        ));
        assert_eq!(incomplete.remedy, Some(Remedy::RetryFailedRefreshes));

        for f in [
            check_domain_identity(Some("old"), &served(Some("new"))),
            check_domain_identity(Some("a"), &DocumentRead::Unheard),
            check_fetch_failure_by_peer(&ledger(
                vec![rec("p", "apps", RefreshOutcome::Withheld)],
                1,
            )),
            check_catalog_completeness(&ledger(vec![rec("p", "apps", RefreshOutcome::Current)], 1)),
        ] {
            assert_eq!(
                f.remedy, None,
                "{:?}/{:?} offers a repair this app cannot perform",
                f.check, f.verdict
            );
        }
    }

    const ALL_REMEDIES: &[Remedy] = &[Remedy::RetryFailedRefreshes];

    /// **A healthy profile must show nothing under a heading called
    /// "Problems".** Measured on the running app: with no deployment document
    /// and no launcher opened, all three checks legitimately come back
    /// `SourceSilent`/`NothingToCheck` — nothing is wrong — and the first
    /// implementation listed all three as findings. A screen that reports
    /// non-problems as problems is one people stop reading.
    #[test]
    fn nothing_to_compare_is_not_a_problem_and_a_divergence_always_is() {
        assert!(!Verdict::SourceSilent.warrants_attention());
        assert!(!Verdict::NothingToCheck.warrants_attention());
        assert!(!Verdict::Agrees.warrants_attention());
        assert!(Verdict::Diverges.warrants_attention());
        // A check that TRIED and could not tell is reported. It is the state
        // that must never be able to pass as health, so it cannot be silent.
        assert!(Verdict::Undetermined.warrants_attention());

        // The whole ordinary-healthy shape: three checks, nothing reportable.
        let findings = run_checks(None, &DocumentRead::NoDocument { status: 404 }, &ledger(vec![], 0));
        assert_eq!(findings.len(), 3);
        assert_eq!(
            findings.iter().filter(|f| f.verdict.warrants_attention()).count(),
            0,
            "an untouched, healthy profile put {:?} in front of the user",
            findings
                .iter()
                .filter(|f| f.verdict.warrants_attention())
                .map(|f| (f.check.key(), f.verdict.label()))
                .collect::<Vec<_>>()
        );
    }

    /// …and the section must not go silent when something IS wrong. The twin of
    /// the test above: a quiet-by-default surface is only safe if the loud path
    /// still fires.
    #[test]
    fn a_real_divergence_is_always_put_in_front_of_the_user() {
        let findings = run_checks(
            Some("peerOLD"),
            &served(Some("peerNEW")),
            &ledger(vec![rec("peerOLD", "games", RefreshOutcome::Withheld)], 1),
        );
        let shown: Vec<&str> = findings
            .iter()
            .filter(|f| f.verdict.warrants_attention())
            .map(|f| f.check.key())
            .collect();
        assert!(shown.contains(&"domain-identity"), "the re-key was not reported: {shown:?}");
        assert!(
            shown.contains(&"fetch-failure-by-peer"),
            "the dead publisher was not reported: {shown:?}"
        );
    }

    /// The chip words must be distinct from each other AND from the log labels.
    /// If a chip ever equals a label, a reworded chip silently changes a log
    /// field and a grep for an incident stops finding it.
    #[test]
    fn the_chip_words_are_distinct_and_are_not_the_log_labels() {
        use std::collections::BTreeSet;
        let all = [
            Verdict::Diverges,
            Verdict::Undetermined,
            Verdict::SourceSilent,
            Verdict::NothingToCheck,
            Verdict::Agrees,
        ];
        let chips: BTreeSet<&str> = all.iter().map(|v| v.chip()).collect();
        assert_eq!(chips.len(), all.len(), "two verdicts render the same chip");
        for v in all {
            assert_ne!(
                v.chip(),
                v.label(),
                "{:?}'s display text and its stable log token are the same string — \
                 rewording the chip would silently move the log field",
                v
            );
        }
    }

    /// `run_checks` must run **all** of them. A surface cannot be allowed to
    /// see a subset, and a check added to the enum but not to the runner would
    /// be invisible everywhere while looking complete in the source.
    #[test]
    fn the_runner_reports_every_check_exactly_once() {
        use std::collections::BTreeSet;
        let findings = run_checks(Some("a"), &served(Some("a")), &ledger(vec![], 0));
        let seen: BTreeSet<&str> = findings.iter().map(|f| f.check.key()).collect();
        assert_eq!(
            seen.len(),
            findings.len(),
            "the runner reported the same check twice"
        );
        for c in [Check::DomainIdentity, Check::FetchFailureByPeer, Check::CatalogCompleteness] {
            assert!(seen.contains(c.key()), "{} is defined but never run", c.key());
        }
    }

    /// End to end on the incident-B shape: the finding appears, it carries the
    /// repair, applying the repair moves the signal the launcher watches, and
    /// once the retry succeeds the finding clears. **This is the whole loop** —
    /// detect, offer, act, re-report — and it is the half that would otherwise
    /// stay unvalidated until a real incident.
    #[test]
    fn the_incident_b_loop_closes_detect_offer_act_and_clear() {
        crate::refresh_ledger::reset_for_test();

        // One set loaded, one did not — the grid looks complete.
        crate::refresh_ledger::record("pubA", "the apps catalog", RefreshOutcome::Current);
        crate::refresh_ledger::record(
            "pubA",
            "the games catalog",
            RefreshOutcome::Unreachable("gave up after 5 attempts".into()),
        );

        let f = check_catalog_completeness(&crate::refresh_ledger::snapshot());
        assert_eq!(f.verdict, Verdict::Diverges, "the incomplete load was not detected");
        let remedy = f.remedy.expect("the finding must offer the repair");

        let before = crate::refresh_ledger::retry_generation();
        assert_eq!(remedy.apply(), RemedyOutcome::Requested);
        assert!(
            crate::refresh_ledger::retry_generation() > before,
            "the repair did not move the signal the launcher watches"
        );

        // The launcher retries and this time it works.
        crate::refresh_ledger::record("pubA", "the games catalog", RefreshOutcome::Current);
        let after = check_catalog_completeness(&crate::refresh_ledger::snapshot());
        assert_eq!(
            after.verdict,
            Verdict::Agrees,
            "the finding did not clear after the retry succeeded, so the surface would go \
             on reporting a problem that is fixed"
        );
        assert!(after.remedy.is_none(), "a cleared finding must not still offer a repair");
    }

    /// Every check must have its own stable key and its own heading — a gate
    /// against two checks colliding in a log line or a chip.
    #[test]
    fn every_check_has_its_own_key_and_title() {
        let all = [Check::DomainIdentity, Check::FetchFailureByPeer, Check::CatalogCompleteness];
        let keys: std::collections::BTreeSet<&str> = all.iter().map(|c| c.key()).collect();
        let titles: std::collections::BTreeSet<&str> = all.iter().map(|c| c.title()).collect();
        assert_eq!(keys.len(), all.len());
        assert_eq!(titles.len(), all.len());
        assert_eq!(all.len(), 3, "checks 4-8 of design §7.2 are not built; update this count");
    }
}

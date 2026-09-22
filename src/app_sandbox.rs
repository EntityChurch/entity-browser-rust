//! The sandbox trust tiers for an embedded app frame — the one expression of
//! which tokens each delivery gets, and the only one that `make test` can reach.
//!
//! **Why this is not in `dom::games` where it was.** That module is
//! `#[cfg(target_arch = "wasm32")]`, so the token set was reachable by exactly
//! one thing: a Selenium run. It went **two months** missing a token that
//! `entity-apps`' own `EMBEDDING.md` §5 documents as required, with every gate
//! green, because no gate looked. A security decision whose only witness is the
//! browser suite is a decision nobody re-reads.
//!
//! The type lives here too rather than being mirrored: a second enum shadowing
//! `AppDelivery` would be C15's drift bought to avoid a file move. `dom::games`
//! re-exports it, so no call site changed.
//!
//! **The tiers, and what each token is buying:**
//!
//! | delivery | tokens | why |
//! |---|---|---|
//! | [`AppDelivery::Srcdoc`] | `allow-scripts allow-downloads` | third-party bundle, **opaque origin** |
//! | [`AppDelivery::Src`] | `allow-scripts allow-same-origin` | our own L5 payload, needs to load its own wasm |
//!
//! `allow-downloads` permits a download to be **initiated** and nothing else. It
//! does not weaken origin isolation the way `allow-same-origin` does, so the
//! worst it buys a hostile bundle is prompting the user with a file save.
//! Omitting it fails **silently** — the browser blocks the download with no error
//! and no exception, `Entity.Export` cannot detect it, and an export button
//! simply appears to do nothing.
//!
//! ⚠ **It is a mitigation, not the answer.** It puts the bytes in the browser's
//! Downloads folder — which the entity tree cannot read, the peer cannot serve
//! and the run-environment cannot ingest. The answer is a file verb returning
//! them to the host (`ROUTING-2026-09-11-q-entity-apps-…`); when that lands this
//! token stays as the unhosted fallback and stops being the only route out.

/// How the bundle reaches the sandboxed iframe — and, because the two questions
/// have one answer, which trust tier it runs at.
///
/// Most apps are self-contained HTML inlined as `srcdoc`; an L5 app (a WASM
/// entity-peer payload) is a multi-MB wasm that cannot practically be
/// base64-inlined, so it is served from a URL via `src` (review G1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppDelivery {
    /// Inline `bundle_html` as the iframe `srcdoc` (self-contained apps; default).
    Srcdoc,
    /// Load the bundle from `url` via the iframe `src`. Same-origin under `dist/`
    /// in the deployed browser; the loader shell fetches its wasm from there.
    Src(String),
}

/// The `sandbox` attribute for a delivery. **The single expression** — nothing
/// else in the tree may spell these tokens.
pub fn sandbox_tokens(delivery: &AppDelivery) -> &'static str {
    match delivery {
        AppDelivery::Srcdoc => "allow-scripts allow-downloads",
        AppDelivery::Src(_) => "allow-scripts allow-same-origin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token `allow-same-origin` hands the frame our origin and defeats the
    /// whole isolation argument. A third-party bundle must never carry it, and
    /// this is the assertion that would have to be deliberately deleted rather
    /// than merely forgotten.
    #[test]
    fn a_third_party_bundle_never_gets_our_origin() {
        assert!(!sandbox_tokens(&AppDelivery::Srcdoc).contains("allow-same-origin"));
    }

    /// The defect this module was created by: `Entity.Export` is silently dead
    /// without it, in every art app, and **hiding the button is not available to
    /// us** because the button is inside the bundle rather than in our chrome.
    #[test]
    fn a_third_party_bundle_may_initiate_a_download() {
        assert!(sandbox_tokens(&AppDelivery::Srcdoc).contains("allow-downloads"));
    }

    /// Our own payload needs its origin back to load its wasm without the
    /// opaque-origin CORS/CSP friction — and has no measured need to download
    /// (no `download`/`createObjectURL` anywhere in `src/app_host/`), so it does
    /// not get a token on grounds of symmetry with the tier below it.
    #[test]
    fn our_own_payload_gets_its_origin_and_not_a_download_token() {
        let t = sandbox_tokens(&AppDelivery::Src("x".into()));
        assert!(t.contains("allow-same-origin"));
        assert!(!t.contains("allow-downloads"));
    }

    /// A census over the token *vocabulary*, not over either tier: every token
    /// we grant anywhere must be one somebody decided to grant. `allow-forms`,
    /// `allow-popups`, `allow-modals`, `allow-top-navigation` and
    /// `allow-pointer-lock` are each a trust-tier change rather than plumbing,
    /// so a new one has to be added here — with a reason — before it can ship.
    #[test]
    fn no_tier_grants_a_token_nobody_decided_on() {
        const DECIDED: &[&str] = &["allow-scripts", "allow-downloads", "allow-same-origin"];
        for d in [AppDelivery::Srcdoc, AppDelivery::Src("x".into())] {
            for tok in sandbox_tokens(&d).split_whitespace() {
                assert!(
                    DECIDED.contains(&tok),
                    "{tok:?} is granted to {d:?} and is not in the decided set. \
                     Adding a sandbox token is a trust-tier change: put it in \
                     DECIDED with the reason, or take it out."
                );
            }
        }
    }

    /// Both tiers must run scripts at all — the thing every app needs and the
    /// one token whose absence would be loud rather than silent.
    #[test]
    fn every_tier_can_run_scripts() {
        for d in [AppDelivery::Srcdoc, AppDelivery::Src("x".into())] {
            assert!(sandbox_tokens(&d).contains("allow-scripts"), "{d:?}");
        }
    }
}

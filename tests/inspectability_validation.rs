//! Consumer-side validation of the inspectability substrate work
//! that landed in `entity-core-rust` — commits 8bb3c63
//! (A4+A5), fd68143 (B1), e75f572 (R2), 52f204e (A1), aec6ccb (A2),
//! 8e490f6 (B2), e089380 (A3), 1e4ecdf (review pass).
//!
//! **Purpose.** Prove the substrate-level changes behave correctly
//! when exercised through Dom's consumer SDK surface. Not a re-test
//! of Core Rust's own per-commit tests; this validates the *consumer
//! pipeline* — that what the substrate enforces is observable from
//! our SDK-facing surface.
//!
//! Pattern mirrors `tests/sdk_consumer_validation.rs`.
//!
//! **Coverage today (SDK-surface checks):**
//! - **B2** — subscribe to a sensitive-prefix pattern from a non-
//!   operator scope fails with `403` per `GUIDE-CAPABILITIES.md` §10
//!   + audit §2.1. Three families: capability, runtime, continuation.
//!   Plus a positive sanity check: non-sensitive prefixes succeed.
//! - **R2** — execute against a dispatcher path with no registered
//!   handler returns the 404 sync response per INBOX §3.6 option (A);
//!   no silent-drop class.
//!
//! **B1 + Chain Trace pipeline (cache observation of substrate-
//! written markers)** is validated in-binary at
//! `src/chain_trace_cache.rs` — integration tests can't reach our
//! crate's modules because we ship a binary, not a library. See the
//! tests there for the end-to-end marker → cache → window snapshot
//! check.
//!
//! **Not covered here** (waits on SDK boundary work):
//! - A1 dispatch event consumption (no SDK pass-through yet).
//! - A2 subscription emit/deliver hook consumption.
//! - A3 wire hook consumption.

use entity_ecf::{text, to_ecf};
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_sdk::{PeerContext, PeerContextBuilder, SdkError};

fn make_ctx() -> PeerContext {
    PeerContextBuilder::new()
        .generate_keypair()
        .build()
        .expect("PeerContext build should succeed")
}

// =====================================================================
// B2 — subscription refusal on sensitive prefixes
// =====================================================================

/// `system/capability/grants/*` is the canonical attack-scenario
/// prefix from `SECURITY-AUDIT-INSPECTABILITY-BASELINE §5.1`. Default
/// PeerContextBuilder scope is not operator-class — wildcards do not
/// count per Ruling 1 — so the subscribe MUST be refused with 403.
///
/// **SDK-boundary gap observed:** the substrate's
/// `sensitive_path` reason code is dropped in the SDK error mapping;
/// callers see only `Forbidden { status: 403, message: "subscribe:
/// <pattern>" }`. The refusal still happens correctly; the reason
/// is lost. Tracked in the upstream inspect SDK-boundary review
/// (added after this validation surfaced it).
#[tokio::test(flavor = "current_thread")]
async fn b2_subscribe_to_sensitive_capability_prefix_is_refused() {
    let ctx = make_ctx();
    let result = ctx
        .subscribe("system/capability/grants/*", |_event| {})
        .await;
    assert!(
        matches!(result, Err(SdkError::Forbidden { status: 403, .. })),
        "subscribe to system/capability/grants/* MUST be refused with \
         403 — this is the audit §5.1 attack scenario. Got: {:?}",
        result.err(),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn b2_subscribe_to_sensitive_runtime_prefix_is_refused() {
    let ctx = make_ctx();
    let result = ctx
        .subscribe("system/runtime/chain-errors/*", |_event| {})
        .await;
    assert!(
        matches!(result, Err(SdkError::Forbidden { status: 403, .. })),
        "system/runtime/** is local-namespace per audit §2.5 and \
         requires operator-class subscribe; got: {:?}",
        result.err(),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn b2_subscribe_to_sensitive_continuation_prefix_is_refused() {
    let ctx = make_ctx();
    let result = ctx
        .subscribe("system/continuation/*", |_event| {})
        .await;
    assert!(
        matches!(result, Err(SdkError::Forbidden { status: 403, .. })),
        "system/continuation/** is local-namespace per audit §2.5 and \
         requires operator-class subscribe; got: {:?}",
        result.err(),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn b2_subscribe_to_app_prefix_is_permitted() {
    // Sanity check: the refusal is keyed on sensitive system/* families,
    // not blanket. App-tier subscribes still work.
    let ctx = make_ctx();
    let result = ctx
        .subscribe("app/entity-browser/event-log/*", |_event| {})
        .await;
    assert!(
        result.is_ok(),
        "non-sensitive prefix subscription should succeed; got err={:?}",
        result.err(),
    );
}

// =====================================================================
// R2 — dispatcher 404 on handler-not-found (INBOX §3.6 option A)
// =====================================================================

/// Execute to a path with no handler ever registered.
///
/// **Substrate-correct contract** (per Core Rust's Ask (d) follow-up):
/// the SDK's `execute()` returns `Ok(HandlerResult { status: 404,
/// code: Some("handler_not_found"), .. })` — substrate-level 404
/// reaches the consumer with both status and code preserved. The
/// caller decides whether to treat it as an error; the dispatcher's
/// job is to surface it, not coerce it into `Err(_)`.
///
/// What R2 actually ruled out is **silent drop** (option C of §3.6):
/// the caller must see a sync response, never nothing. This test
/// asserts that. It also asserts the substrate `code: "handler_not_found"`
/// reaches the consumer — the Ask (d) fix verified.
#[tokio::test(flavor = "current_thread")]
async fn r2_execute_to_missing_handler_returns_sync_404_with_code() {
    let ctx = make_ctx();
    let target = "definitely/no/handler/here";
    let params = Entity::new("system/protocol/request", to_ecf(&text("ignored"))).unwrap();
    let result = ctx
        .execute(target, "any-op", params, ExecuteOptions::default())
        .await
        .expect("R2 forbids silent drop — execute must return a sync response");

    assert_eq!(
        result.status, 404,
        "expected 404 sync response per INBOX §3.6 option (A); status was: {}",
        result.status,
    );

    // Ask (d) — substrate code must reach the consumer through the
    // SDK error envelope, not be stringified away.
    let decoded = entity_handler::decode_error_entity(&result.result);
    let (code, message) = decoded.expect(
        "404 response body must be a `system/protocol/error` entity \
         decodable by entity_handler::decode_error_entity",
    );
    assert_eq!(
        code.as_deref(),
        Some("handler_not_found"),
        "substrate `code` field must reach the consumer; got: {code:?}",
    );
    assert!(
        message
            .as_deref()
            .map(|m| m.contains("definitely/no/handler/here") || m.contains("no handler"))
            .unwrap_or(false),
        "error message should identify the missing path; got: {message:?}",
    );
}

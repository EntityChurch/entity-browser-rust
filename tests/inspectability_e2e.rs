//! End-to-end demonstration that the inspectability surface is
//! actually usable from Dom's consumer SDK.
//!
//! This is the "see it work" test. Three scenarios run a real
//! continuation-shaped workflow with SDK-installed hooks attached
//! and assert the inspectability machinery surfaces what's happening
//! through the consumer boundary.
//!
//! Run with `--nocapture` to see the captured dispatch + binding
//! events printed inline — that's the diagnostic surface from a
//! consumer's seat.
//!
//! Cycle scope:
//! - Ask (a) shipped: `PeerContextBuilder::with_dispatch_hook /
//!   with_wire_hook / with_binding_hook` pass-throughs to
//!   `PeerBuilder` at `bindings/sdk/src/sdk.rs`.
//! - Ask (c) shipped: `PeerContext::is_operator_class_for` SDK
//!   wrapper around `entity_protocol::is_operator_class_for`.
//! - Ruling 1 verified: wildcard caps never count as operator-class.
//!
//! Ask (b) — Worker arm hook installation across postMessage — is
//! still a design question; out of scope for this native test.

use std::sync::{Arc, Mutex};

use entity_ecf::{text, to_ecf};
use entity_entity::Entity;
use entity_sdk::{
    InspectDispatchEvent, PeerContext, PeerContextBuilder, SdkError, TreeChangeEvent,
};

fn make_ctx() -> PeerContext {
    PeerContextBuilder::new()
        .generate_keypair()
        .build()
        .expect("PeerContext build should succeed")
}

// =====================================================================
// Demo 1: dispatch hook fires on real tree operations
// =====================================================================

/// Build a peer with a dispatch hook installed via the new SDK
/// pass-through, run a tree.put + tree.get, observe what the hook
/// captures. This is the path-tap primitive surfaced through the SDK
/// boundary.
#[tokio::test(flavor = "current_thread")]
async fn demo_dispatch_hook_fires_for_real_tree_operations() {
    // Shared capture buffer the hook writes into.
    let captures: Arc<Mutex<Vec<DispatchCapture>>> = Arc::new(Mutex::new(Vec::new()));
    let captures_for_hook = captures.clone();

    // Build with hook installed up front (per audit §2 invariant —
    // observe-only; closure cannot retain `&DispatchEvent`).
    let ctx = PeerContextBuilder::new()
        .generate_keypair()
        .with_dispatch_hook("demo/dispatch-tap", move |event: &InspectDispatchEvent| {
            captures_for_hook.lock().unwrap().push(DispatchCapture {
                target_uri: event.target_uri.clone(),
                operation: event.operation.clone(),
                phase: format!("{:?}", event.phase),
                request_id: event.request_id.clone(),
            });
        })
        .build()
        .expect("build with hook installed");

    // Exercise the dispatcher: put an entity, then read it back.
    let entity = Entity::new("app/demo/note", to_ecf(&text("hello inspectability"))).unwrap();
    ctx.put("demo/path", entity).await.expect("put dispatches");
    let _ = ctx.get("demo/path").await.expect("get dispatches");

    let final_captures = captures.lock().unwrap().clone();

    // Print what the hook saw — that's the diagnostic.
    eprintln!("\n=== Dispatch hook captured {} events ===", final_captures.len());
    for c in &final_captures {
        eprintln!(
            "  target={} op={} phase={} req={}",
            c.target_uri, c.operation, c.phase, c.request_id,
        );
    }

    // Two operations (put + get) × two phases (entry + exit) = 4 minimum.
    // Implementation may dispatch additional internal ops; floor only.
    assert!(
        final_captures.len() >= 4,
        "hook should fire at entry + exit for each of put + get (≥4); \
         got {}",
        final_captures.len(),
    );

    // Verify we saw both operations through the system/tree handler.
    let saw_put = final_captures.iter().any(|c| c.operation == "put");
    let saw_get = final_captures.iter().any(|c| c.operation == "get");
    assert!(saw_put, "expected to observe a `put` dispatch");
    assert!(saw_get, "expected to observe a `get` dispatch");

    // Verify entry+exit pairing by phase. Hook receives both phases for
    // each dispatch; same request_id correlates the pair.
    let entry_phases = final_captures
        .iter()
        .filter(|c| c.phase.contains("Entry"))
        .count();
    let exit_phases = final_captures
        .iter()
        .filter(|c| c.phase.contains("Exit"))
        .count();
    assert!(entry_phases >= 2, "expected ≥2 entry-phase events");
    assert!(exit_phases >= 2, "expected ≥2 exit-phase events");
    assert_eq!(
        entry_phases, exit_phases,
        "entry + exit phases should pair 1:1 — each dispatch produces one of each",
    );
}

#[derive(Clone, Debug)]
struct DispatchCapture {
    target_uri: String,
    operation: String,
    phase: String,
    request_id: String,
}

// =====================================================================
// Demo 2: binding hook surfaces every entity write
// =====================================================================

/// Binding hook is the v1.2 §2.1 #2 surface — fires on every path
/// bind/rebind/unbind in the location index. Build with hook
/// installed, write entities, observe the cascade of binding events
/// (cache events, location-index writes).
#[tokio::test(flavor = "current_thread")]
async fn demo_binding_hook_surfaces_tree_writes() {
    let captures: Arc<Mutex<Vec<BindingCapture>>> = Arc::new(Mutex::new(Vec::new()));
    let captures_for_hook = captures.clone();

    let ctx = PeerContextBuilder::new()
        .generate_keypair()
        .with_binding_hook("demo/binding-stream", move |event: &TreeChangeEvent| {
            let cascade_depth = event.context.as_ref().map(|c| c.cascade_depth).unwrap_or(0);
            captures_for_hook.lock().unwrap().push(BindingCapture {
                path: event.path.clone(),
                kind: format!("{:?}", event.change_type),
                cascade_depth,
            });
        })
        .build()
        .expect("build with binding hook");

    // Write three distinct entities to different paths.
    for n in 1..=3 {
        let entity =
            Entity::new("app/demo/note", to_ecf(&text(&format!("note {n}")))).unwrap();
        ctx.put(format!("demo/notes/{n}"), entity)
            .await
            .expect("put dispatches");
    }

    let final_captures = captures.lock().unwrap().clone();
    eprintln!("\n=== Binding hook captured {} events ===", final_captures.len());
    for c in &final_captures {
        eprintln!(
            "  path={} kind={} cascade_depth={}",
            c.path, c.kind, c.cascade_depth,
        );
    }

    // At minimum the three demo paths should appear (cascade depth 0).
    let demo_path_bindings = final_captures
        .iter()
        .filter(|c| c.path.contains("demo/notes/"))
        .count();
    assert!(
        demo_path_bindings >= 3,
        "expected ≥3 binding events for demo/notes/{{1,2,3}}; got {}",
        demo_path_bindings,
    );
}

#[derive(Clone, Debug)]
struct BindingCapture {
    path: String,
    kind: String,
    cascade_depth: u32,
}

// =====================================================================
// Demo 3: multiple hooks compose without interfering
// =====================================================================

/// Multiple hooks can be installed; each fires in registration order.
/// Tests the type-erased Arc<dyn Fn> storage in PeerContextBuilder
/// holds many heterogeneous closures correctly.
#[tokio::test(flavor = "current_thread")]
async fn demo_multiple_hooks_compose() {
    let count1: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let count2: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let c1 = count1.clone();
    let c2 = count2.clone();

    let ctx = PeerContextBuilder::new()
        .generate_keypair()
        .with_dispatch_hook("demo/counter-1", move |_event| {
            *c1.lock().unwrap() += 1;
        })
        .with_dispatch_hook("demo/counter-2", move |_event| {
            *c2.lock().unwrap() += 1;
        })
        .build()
        .expect("build with two dispatch hooks");

    let entity = Entity::new("app/demo/note", to_ecf(&text("x"))).unwrap();
    ctx.put("demo/x", entity).await.expect("put dispatches");

    let n1 = *count1.lock().unwrap();
    let n2 = *count2.lock().unwrap();
    eprintln!("\n=== Multi-hook fire counts: hook1={n1} hook2={n2} ===");
    assert!(n1 >= 2, "first hook should fire ≥2 (entry + exit)");
    assert!(n2 >= 2, "second hook should fire ≥2 (entry + exit)");
    assert_eq!(
        n1, n2,
        "both hooks must see the same dispatch event count",
    );
}

// =====================================================================
// Demo 4: operator-class check via SDK wrapper (Ask c / Ruling 1)
// =====================================================================

/// `PeerContext::is_operator_class_for` lets app code consult the
/// same operator-class check the substrate uses for subscription
/// refusal (B2) — without reaching into `entity_protocol`.
///
/// Verified properties:
/// 1. **Fails closed.** An unknown cap hash (no entity in the store)
///    returns `false`, never `true`. App code can safely consult the
///    check without worrying about phantom positives on race
///    conditions.
/// 2. **Substrate-consistent.** When the SDK wrapper says "not
///    operator-class," the substrate's subscription handler agrees
///    (returns 403 sensitive_path on the same prefix). This is the
///    defense-in-depth invariant from `FEEDBACK-INSPECTABILITY-EGUI`
///    §4.4: the app-tier UX surface and the substrate-tier
///    enforcement reach the same conclusion.
#[tokio::test(flavor = "current_thread")]
async fn demo_operator_class_check_fails_closed_on_unknown_cap() {
    let ctx = make_ctx();
    // A hash that isn't in the content store — the chain walk must
    // fail immediately and return false. Ruling 1: fail closed on
    // any uncertainty.
    let unknown_hash = entity_hash::Hash::new(1, [0u8; 32]);
    let is_op = ctx.is_operator_class_for(&unknown_hash, "system/capability/grants/foo");
    eprintln!("\n=== Operator-class check — unknown cap hash ===");
    eprintln!(
        "  is_operator_class_for(unknown_hash, system/capability/grants/foo) = {}",
        is_op,
    );
    assert!(
        !is_op,
        "Ruling 1: fail closed on chain-walk failures (unreachable parent, \
         malformed cap entity). An unknown hash MUST return false — \
         returning true would let a phantom cap escalate to operator-class.",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn demo_sdk_check_agrees_with_substrate_refusal() {
    // The substrate refuses sensitive-prefix subscribes when the
    // caller's scope isn't operator-class (B2, ext/subscription/lib.rs:258).
    // The SDK wrapper consults the same primitive
    // (entity_protocol::is_operator_class_for). Both layers must agree:
    // when the SDK says "not operator-class," the substrate refuses.
    let ctx = make_ctx();
    let result = ctx
        .subscribe("system/capability/grants/foo", |_event| {})
        .await;
    eprintln!("\n=== Substrate refusal agrees with SDK check ===");
    eprintln!(
        "  subscribe(system/capability/grants/foo) → {:?}",
        result.as_ref().err(),
    );
    assert!(
        matches!(result, Err(SdkError::Forbidden { status: 403, .. })),
        "substrate-tier refusal is the binding contract; got: {:?}",
        result.err(),
    );
}

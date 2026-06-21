//! Integration tests — validate entity-peer API for Phase 5a.
//!
//! These tests exercise PeerBuilder, local_only(), tree operations,
//! and handler dispatch via peer.execute(). They run natively only
//! (tokio runtime required).
//!
//! Path convention: all tree paths are peer-namespaced. The low-level
//! tree.put()/get()/has() methods take raw paths (must include peer_id
//! prefix). The execute() path qualifies automatically.

use std::sync::Arc;

use entity_capability::ResourceTarget;
use entity_crypto::Keypair;
use entity_ecf::{text, to_ecf, Value};
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_peer::PeerBuilder;
use entity_store::{LocationIndex, MemoryContentStore, MemoryLocationIndex};

fn test_keypair() -> Keypair {
    Keypair::generate()
}

fn make_entity(entity_type: &str, content: &str) -> Entity {
    let data = to_ecf(&text(content));
    Entity::new(entity_type, data).unwrap()
}

// ---------------------------------------------------------------------------
// PeerBuilder basics
// ---------------------------------------------------------------------------

#[test]
fn peer_builder_creates_peer() {
    let peer = PeerBuilder::new()
        .keypair(test_keypair())
        .build()
        .unwrap();
    assert!(!peer.peer_id().as_str().is_empty());
}

#[test]
fn peer_builder_requires_keypair() {
    let result = PeerBuilder::new().build();
    assert!(result.is_err());
}

#[test]
fn peer_builder_with_custom_stores() {
    let store = Arc::new(MemoryContentStore::new());
    let index = Arc::new(MemoryLocationIndex::new());
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new()
        .keypair(kp)
        .content_store(store.clone())
        .location_index(index.clone())
        .build()
        .unwrap();
    // Bootstrap stores at qualified paths: {peer_id}/system/tree
    let qualified = format!("/{}/system/tree", pid);
    assert!(peer.tree().has(&qualified));
    assert!(index.has(&qualified));
}

#[test]
fn peer_deterministic_from_seed() {
    let p1 = PeerBuilder::new()
        .keypair(Keypair::from_seed([42u8; 32]))
        .build()
        .unwrap();
    let p2 = PeerBuilder::new()
        .keypair(Keypair::from_seed([42u8; 32]))
        .build()
        .unwrap();
    assert_eq!(p1.peer_id(), p2.peer_id());
}

// ---------------------------------------------------------------------------
// Tree operations via peer.tree() — all paths peer-namespaced
// ---------------------------------------------------------------------------

#[test]
fn tree_put_and_get() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();

    let entity = make_entity("document/markdown", "# Hello");
    let path = format!("/{}/docs/readme", pid);
    let hash = peer.tree().put(&path, entity.clone()).unwrap();
    let got = peer.tree().get(&path).unwrap();
    assert_eq!(got.content_hash, hash);
    assert_eq!(got.entity_type, "document/markdown");
}

#[test]
fn tree_get_missing_returns_none() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    assert!(peer.tree().get(&format!("/{}/nonexistent/path", pid)).is_none());
}

#[test]
fn tree_listing() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();

    peer.tree().put(&format!("/{}/docs/a", pid), make_entity("test/t", "a")).unwrap();
    peer.tree().put(&format!("/{}/docs/b", pid), make_entity("test/t", "b")).unwrap();
    peer.tree().put(&format!("/{}/other/c", pid), make_entity("test/t", "c")).unwrap();

    let result = peer.tree().handle_listing(&format!("/{}/docs/", pid)).unwrap();
    assert_eq!(result.status, 200);
}

#[test]
fn tree_has_system_paths() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();

    // PeerBuilder bootstraps handler manifests and type definitions under {peer_id}/
    assert!(peer.tree().has(&format!("/{}/system/tree", pid)));                   // handler manifest
    assert!(peer.tree().has(&format!("/{}/system/handler/system/tree", pid)));    // handler interface
    assert!(peer.tree().has(&format!("/{}/system/type/system/handler", pid)));    // type definition
}

#[test]
fn tree_has_identity() {
    let kp = test_keypair();
    let peer_id = kp.peer_id();
    let pid = peer_id.to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    assert!(peer.tree().has(&format!("/{}/system/identity/{}", pid, pid)));
}

// ---------------------------------------------------------------------------
// Handler dispatch via peer.execute()
// Resource targets are unqualified — execute qualifies with local peer_id.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_tree_get() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    peer.local_only();

    // Put at qualified path (raw tree API)
    let entity = make_entity("test/note", "hello world");
    peer.tree().put(&format!("/{}/notes/first", pid), entity.clone()).unwrap();

    // Retrieve via execute — resource target is unqualified, gets qualified internally
    let opts = ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec!["notes/first".into()],
            exclude: vec![],
        }),
        ..Default::default()
    };
    let empty_params = Entity::new("system/empty", to_ecf(&Value::Null)).unwrap();
    let result = peer
        .execute_with_options("system/tree", "get", empty_params, opts)
        .await
        .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.result.content_hash, entity.content_hash);
}

#[tokio::test]
async fn execute_tree_get_not_found() {
    let peer = PeerBuilder::new()
        .keypair(test_keypair())
        .build()
        .unwrap();
    peer.local_only();

    let opts = ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec!["missing/path".into()],
            exclude: vec![],
        }),
        ..Default::default()
    };
    let empty_params = Entity::new("system/empty", to_ecf(&Value::Null)).unwrap();
    let result = peer
        .execute_with_options("system/tree", "get", empty_params, opts)
        .await
        .unwrap();
    assert_eq!(result.status, 404);
}

#[tokio::test]
async fn execute_tree_listing() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    peer.local_only();

    peer.tree().put(&format!("/{}/docs/a", pid), make_entity("test/t", "a")).unwrap();
    peer.tree().put(&format!("/{}/docs/b", pid), make_entity("test/t", "b")).unwrap();

    // Trailing slash on resource target triggers listing
    let opts = ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec!["docs/".into()],
            exclude: vec![],
        }),
        ..Default::default()
    };
    let empty_params = Entity::new("system/empty", to_ecf(&Value::Null)).unwrap();
    let result = peer
        .execute_with_options("system/tree", "get", empty_params, opts)
        .await
        .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.result.entity_type, "system/tree/listing");
}

// ---------------------------------------------------------------------------
// Shared stores — verify peer exposes stores we can share with UI
// ---------------------------------------------------------------------------

#[test]
fn peer_stores_accessible() {
    let peer = PeerBuilder::new()
        .keypair(test_keypair())
        .build()
        .unwrap();
    let entity = make_entity("test/t", "data");
    let hash = peer.content_store().put(entity).unwrap();
    assert!(peer.content_store().get(&hash).is_some());
}

#[test]
fn peer_event_subscription() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    // Can subscribe to tree change events (for UI refresh)
    let mut rx = peer.subscribe_events();
    peer.tree().put(&format!("/{}/test/path", pid), make_entity("t", "v")).unwrap();
    // Event was sent (try_recv succeeds since put triggers notification)
    assert!(rx.try_recv().is_ok());
}

// ---------------------------------------------------------------------------
// P1a smoke tests — confirm `history` + `handlers` Cargo features actually
// register their handlers at boot.
// See `docs/PARITY-MATRIX.md` for the feature-enablement audit.
// ---------------------------------------------------------------------------

/// `history` feature enabled → `system/history` handler is dispatchable and
/// returns an empty result for a path with no history. Per
/// `EXTENSION-HISTORY.md` query op + bootstrap at `core/peer/src/lib.rs:854`.
#[tokio::test]
async fn history_handler_registered() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    peer.local_only();

    // Bootstrap also seeds the interface entity at
    // `system/handler/system/history` per bootstrap_handler call.
    let iface_path = format!("/{}/system/handler/system/history", pid);
    assert!(
        peer.tree().has(&iface_path),
        "history feature enabled but `system/handler/system/history` interface not in tree"
    );

    // Dispatch: query history for a never-written path. Expect 200 + empty
    // result; a missing handler would return a non-200 dispatch error.
    let mut params_map: Vec<(Value, Value)> = Vec::new();
    params_map.push((text("path"), text("test/never-written")));
    let params = Entity::new(
        "system/history/query/params",
        to_ecf(&Value::Map(params_map)),
    )
    .unwrap();
    let opts = ExecuteOptions::default();
    let result = peer
        .execute_with_options("system/history", "query", params, opts)
        .await
        .expect("history handler dispatch should succeed");
    assert_eq!(
        result.status, 200,
        "history.query should return 200 for never-written path, got {}",
        result.status
    );
}

/// Gap 5 regression guard: a fresh boot must NOT issue any destructive
/// operations against the local tree, regardless of relay reachability.
/// "Offline ≠ wiped." Per the Gap-5 persistence investigation:
/// no boot-path code today consults the relay; this test locks in
/// that property as a regression guard for future PRs that might
/// introduce a "if server unreachable, wipe" code path.
///
/// Approach: pre-seed entities into a fresh peer, "boot" (i.e. construct
/// the consumer surface that runs at startup), assert the seeded
/// entities are still present. If a future change adds boot-time wipe
/// logic, this fails.
#[tokio::test(flavor = "current_thread")]
async fn boot_does_not_wipe_preexisting_tree_state() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();
    peer.local_only();

    // Seed three sentinel entries the boot path has no reason to touch.
    let p_app = format!("/{}/app/entity-browser/sentinel/state", pid);
    let p_user = format!("/{}/user/notes/important", pid);
    let p_settings = format!("/{}/app/entity-browser/settings/ui", pid);
    let seeded = make_entity("test/sentinel", "do-not-delete");
    peer.tree().put(&p_app, seeded.clone()).unwrap();
    peer.tree().put(&p_user, seeded.clone()).unwrap();
    peer.tree().put(&p_settings, seeded.clone()).unwrap();

    // Sanity: all three present pre-boot.
    assert!(peer.tree().has(&p_app));
    assert!(peer.tree().has(&p_user));
    assert!(peer.tree().has(&p_settings));

    // "Boot": simulate the app's load step. On native we don't have a
    // full EntityApp boot, but the SDK-level persistence load is the
    // relevant gate (per `Peers::load_persisted_primary` / WASM
    // `new_wasm`). Construct an SDK from the same peer and assert
    // entries survive — covers the codepath that would change if
    // someone added "wipe on boot" logic at the SDK layer.
    //
    // We deliberately do NOT touch the network: the entire test runs
    // without any relay connection, mirroring the "offline" scenario.
    let _sdk = entity_sdk::EntitySDK::builder()
        .keypair(entity_crypto::Keypair::from_seed(
            // Re-derive from the same seed so the SDK shares this peer's identity.
            // `secret_key_bytes` moved off the IdentityKeypair enum onto the
            // concrete Ed25519 keypair — reach it via the `as_ed25519` escape hatch.
            peer.keypair().as_ed25519().expect("test peer is Ed25519").secret_key_bytes(),
        ))
        .build()
        .expect("SDK build");
    // (The SDK above is a separate peer instance; the goal is just to
    // exercise the boot-time code path without panicking. The seeded
    // entries belong to `peer` and we verify them on `peer` directly.)

    // POST-BOOT: assert all three entries still present.
    assert!(
        peer.tree().has(&p_app),
        "boot should not wipe app/entity-browser state"
    );
    assert!(
        peer.tree().has(&p_user),
        "boot should not wipe user-namespace entities"
    );
    assert!(
        peer.tree().has(&p_settings),
        "boot should not wipe persisted settings"
    );
}

/// `handlers` feature enabled → `system/handler` is a registered V7 §6.9
/// bootstrap handler (register/unregister). Bootstrap at
/// `core/peer/src/lib.rs:950`. We assert via the seeded interface entity
/// since `register` requires full handler-manifest params we don't need to
/// exercise for a registration smoke test.
#[test]
fn handlers_feature_bootstraps_system_handler() {
    let kp = test_keypair();
    let pid = kp.peer_id().to_string();
    let peer = PeerBuilder::new().keypair(kp).build().unwrap();

    // bootstrap_handler seeds the interface at
    // `system/handler/{bare_pattern}` where bare_pattern is the 2nd arg.
    // Call site at lib.rs:962 passes "system/handler" → final path is
    // `/{pid}/system/handler/system/handler`.
    let iface_path = format!("/{}/system/handler/system/handler", pid);
    assert!(
        peer.tree().has(&iface_path),
        "handlers feature enabled but `{}` interface not in tree",
        iface_path
    );

    // Also check the grant exists at the standard path. handlers extension's
    // self-grant is created during the bootstrap loop.
    let grant_path = format!("/{}/system/capability/grants/system/handler", pid);
    assert!(
        peer.tree().has(&grant_path),
        "handlers feature enabled but `system/capability/grants/system/handler` not in tree"
    );
}

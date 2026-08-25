//! Headless verification of the increment-3 **authorize gate mechanism** — the
//! peer-to-peer half the Peer Connections window will drive
//! (`DESIGN-AUTHORIZE-GATE-INCREMENT-3`). Runs as a native cross-peer
//! integration test (real WebSocket, two peers) because the Tauri WebView path
//! needs webkit2gtk + a display; the substrate mechanism does not. Mirrors the
//! cross-peer harness in `tests/local_files_write_persistence.rs`.
//!
//! Proves:
//!   1. **`session_entity_keyed_by_identity_hash_hex`** — B writes the session
//!      entity for a connecting peer A keyed by A's identity-hash HEX, not A's
//!      Base58 PeerID. This resolves the keying question the window's pending
//!      derivation depends on (`peer_auth.rs` contract, design §2.0): `session/*`
//!      and `policy/*` share the hex key space; the app's base58 ids do not.
//!   2. **`unauthorized_denied_then_policy_grant_allows`** — with
//!      `debug_open_grants` OFF, A is DENIED `local/files` until a policy grant
//!      for A is authored on B; after A re-handshakes, it is ALLOWED. The whole
//!      gate end to end (the enforcement steps 2 seeds and step 7 turns on).
//!
//! Native only; `sqlite`/`websocket` come from the test-only dev-dependency.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Arc;

use entity_capability::{encode_grant_entry, GrantEntry, IdScope, PathScope, ResourceTarget};
use entity_crypto::Keypair;
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_peer::local_files::RootConfigData;
use entity_peer::transport::{Connector, WebSocketConnector, WebSocketListener};
use entity_peer::{PeerBuilder, PeerConfig, PeerShared};

/// Mirrors `entity_types::TYPE_CAP_POLICY_ENTRY` (entity-types isn't an app dep;
/// see `decode_policy_grants_at`, `core/peer/src/connection.rs`).
const POLICY_TYPE: &str = "system/capability/policy-entry";

fn empty_params() -> Entity {
    Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap()
}

/// A file-transfer policy grant for `grantee` over B's `shared` root: the
/// `local/files` handler, `list`+`read`, on B's share subtree. Body shape is
/// what `decode_policy_grants_at` reads (`grants` array) + the `peer_pattern` the
/// `configure` writer needs.
fn file_transfer_policy(pid_b: &str, grantee: &str) -> Entity {
    let grant = GrantEntry {
        handlers: PathScope::new(vec!["local/files".into()]),
        resources: PathScope::new(vec![format!("/{}/local/files/shared/*", pid_b)]),
        operations: IdScope::new(vec!["list".into(), "read".into()]),
        peers: None,
        constraints: None,
        allowances: None,
    };
    let arr = vec![encode_grant_entry(&grant)];
    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
        (entity_ecf::text("grants"), entity_ecf::Value::Array(arr)),
        (entity_ecf::text("peer_pattern"), entity_ecf::text(grantee)),
    ]));
    Entity::new(POLICY_TYPE, data).unwrap()
}

/// A wildcard policy grant (mirrors `debug_open_grants`: handler `*`, resources
/// `/*/*`, ops `*`, peers `*`). Diagnostic: if THIS confers via the policy table
/// but `file_transfer_policy` does not, the scope is the bug, not the union.
fn wildcard_policy(grantee: &str) -> Entity {
    let grant = GrantEntry {
        handlers: PathScope::new(vec!["*".into()]),
        resources: PathScope::new(vec!["/*/*".into()]),
        operations: IdScope::new(vec!["*".into()]),
        peers: Some(IdScope::new(vec!["*".into()])),
        constraints: None,
        allowances: None,
    };
    let arr = vec![encode_grant_entry(&grant)];
    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
        (entity_ecf::text("grants"), entity_ecf::Value::Array(arr)),
        (entity_ecf::text("peer_pattern"), entity_ecf::text(grantee)),
    ]));
    Entity::new(POLICY_TYPE, data).unwrap()
}

/// Build backend peer B: writable `shared` root + a real WS listener. Returns
/// `(shared_b, pid_b, ws_addr, tmp_root)` — hold `tmp_root` for the test's life.
async fn build_backend(debug_open: bool) -> (Arc<PeerShared>, String, String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let kp_b = Keypair::from_seed([9u8; 32]);
    let pid_b = kp_b.peer_id().to_string();
    let peer_b = PeerBuilder::new()
        .keypair(kp_b)
        .config(PeerConfig { debug_open_grants: debug_open, ..PeerConfig::default() })
        .build()
        .expect("peer B builds");
    let shared_b = peer_b.shared();
    peer_b.start_engines(&shared_b);
    peer_b
        .local_files_handler()
        .add_root(
            "shared",
            RootConfigData {
                prefix: "local/files/shared/".to_string(),
                filesystem_root: tmp.path().to_string_lossy().into_owned(),
                read_only: false,
                ..Default::default()
            },
        )
        .expect("add root");

    let shared_b_run = shared_b.clone();
    let listener = WebSocketListener::bind("127.0.0.1:0").await.expect("ws bind");
    let addr = format!("ws://{}", listener.socket_addr());
    tokio::spawn(async move {
        let _ = entity_peer::server::run(listener, shared_b_run).await;
    });
    (shared_b, pid_b, addr, tmp)
}

/// Build a client peer A (its own grants are irrelevant — B enforces). Returns
/// its persistent `shared` (the single-`shared` discipline; see the harness note
/// in `local_files_write_persistence.rs`).
fn build_client(seed: u8) -> Arc<PeerShared> {
    let peer_a = PeerBuilder::new()
        .keypair(Keypair::from_seed([seed; 32]))
        // A runs with debug_open_grants like the real app peer (the SDK's
        // PeerManager default) and the working cross-peer harness — this governs
        // what A may INITIATE client-side; B still enforces its own grants on A.
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>)
        .build()
        .expect("peer A builds");
    let shared_a = peer_a.shared();
    peer_a.start_engines(&shared_a);
    shared_a
}

/// Dial + handshake A→B against A's persistent shared, pooling the connection
/// (exactly `PeerContext::connect_to`). A fresh call re-handshakes, re-minting
/// A's capability with any policy authored since.
async fn dial(shared_a: &Arc<PeerShared>, addr: &str) -> String {
    let conn = shared_a.connector.connect(addr).await.expect("A dials B");
    let remote = entity_peer::remote::perform_connect_with_dispatch(
        conn,
        &shared_a.keypair,
        shared_a.config.home_hash_format,
        Some(shared_a.clone()),
    )
    .await
    .expect("A handshakes B");
    let pid = remote.remote_peer_id.clone();
    shared_a.remote.insert(&pid, remote);
    pid
}

/// Probe `local/files:list` on B's share from A. Returns the status, or `None`
/// when dispatch itself denied (capability failure surfaced as an error).
async fn probe_list(shared_a: &Arc<PeerShared>, pid_b: &str) -> Option<u32> {
    let execute_fn = entity_peer::connection::make_execute_fn(
        shared_a.clone(),
        Some(shared_a.identity_hash),
        HashMap::new(),
        None,
        None,
    );
    let opts = ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec![format!("/{}/local/files/shared/", pid_b)],
            exclude: vec![],
        }),
        ..Default::default()
    };
    match execute_fn(
        format!("entity://{}/local/files", pid_b),
        "list".to_string(),
        empty_params(),
        opts,
    )
    .await
    {
        Ok(res) => Some(res.status),
        Err(_) => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_entity_keyed_by_identity_hash_hex() {
    let (shared_b, pid_b, addr, _tmp) = build_backend(true).await;
    let shared_a = build_client(1);
    dial(&shared_a, &addr).await;

    let sessions = shared_b
        .tree
        .list(&format!("/{}/system/peer/session/", pid_b));
    assert_eq!(sessions.len(), 1, "exactly one session for the one client");

    let key = sessions[0].path.rsplit('/').next().unwrap();
    assert_eq!(
        key,
        shared_a.identity_hash.to_hex(),
        "session must be keyed by A's identity-hash hex"
    );
    let a_base58 = shared_a.keypair.peer_id().to_string();
    assert_ne!(
        key, a_base58,
        "session key is hex, NOT the Base58 PeerID — the window must reconcile"
    );
}

/// A file-transfer policy grant authored for A on B **before A's first
/// handshake** (the realistic posture — step 2 seeds the manager grant at backend
/// start; an authorize step writes the grant then A connects/reconnects fresh)
/// must confer file access. Run for both grantee key forms so the base58-vs-hex
/// keying question — which step 2's manager grant depends on — is answered
/// definitively, not assumed.
///
/// NB: authoring must precede the handshake. B mints A's capability (with the
/// §4.4 policy union) at authenticate; a policy written to a peer with an already
/// established session is NOT retroactively applied to that live cap — the peer
/// must handshake fresh. (Live-cap push is a documented later enhancement.)
async fn grant_before_handshake_confers(key: GranteeKey) {
    let (shared_b, pid_b, addr, _tmp) = build_backend(false).await;
    let shared_a = build_client(1);

    let grantee = match key {
        GranteeKey::Hex => shared_a.identity_hash.to_hex(),
        GranteeKey::Base58 => shared_a.keypair.peer_id().to_string(),
    };
    shared_b
        .tree
        .put(
            &format!("/{}/system/capability/policy/{}", pid_b, grantee),
            file_transfer_policy(&pid_b, &grantee),
        )
        .expect("author policy grant");

    dial(&shared_a, &addr).await;
    let status = probe_list(&shared_a, &pid_b).await;
    assert_eq!(
        status,
        Some(200),
        "a {:?}-keyed grant authored before handshake must confer local/files",
        key
    );
}

#[derive(Debug, Clone, Copy)]
enum GranteeKey {
    /// A's identity-hash hex — the canonical form the kernel tries first.
    Hex,
    /// A's Base58 PeerID — the pre-connect affordance the kernel self-heals to hex
    /// at handshake. This is the form step 2's manager grant is seeded with.
    Base58,
}

// An unauthorized peer (locked-down B, no policy) must be denied local/files.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_peer_denied() {
    let (_shared_b, pid_b, addr, _tmp) = build_backend(false).await;
    let shared_a = build_client(1);
    dial(&shared_a, &addr).await;
    assert_ne!(
        probe_list(&shared_a, &pid_b).await,
        Some(200),
        "locked-down backend must deny an ungranted peer"
    );
}

// The canonical path: grant keyed by identity-hash hex.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hex_key_grant_confers() {
    grant_before_handshake_confers(GranteeKey::Hex).await;
}

// DIAGNOSTIC: does the policy-table union fire at all? A wildcard policy grant
// (hex-keyed) must confer. 200 ⇒ union works, narrow scope is the bug. 403 ⇒ the
// capability handler still isn't registered / union not firing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wildcard_policy_grant_confers() {
    let (shared_b, pid_b, addr, _tmp) = build_backend(false).await;
    let shared_a = build_client(1);

    // Author the policy BEFORE A's first handshake, so the very first authenticate
    // sees it (this is exactly step 2's posture: the manager grant is seeded at
    // backend start, before S ever connects).
    let a_hex = shared_a.identity_hash.to_hex();
    let policy_path = format!("/{}/system/capability/policy/{}", pid_b, a_hex);
    shared_b
        .tree
        .put(&policy_path, wildcard_policy(&a_hex))
        .expect("author wildcard policy");
    eprintln!(
        "POLICY present={}  handler registered={}",
        shared_b.tree.get(&policy_path).is_some(),
        shared_b
            .handler_registry
            .get(&format!("/{}/system/capability", pid_b))
            .is_some()
    );

    dial(&shared_a, &addr).await;
    assert_eq!(
        probe_list(&shared_a, &pid_b).await,
        Some(200),
        "wildcard policy authored BEFORE first handshake must confer"
    );
}

// The pre-connect affordance: grant keyed by Base58 PeerID. Confirms step 2's
// base58-keyed manager grant will actually confer (it seeds by S's base58 id).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn base58_key_grant_confers() {
    grant_before_handshake_confers(GranteeKey::Base58).await;
}

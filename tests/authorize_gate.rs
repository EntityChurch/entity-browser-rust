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
//!   3. **`manager_authorizes_device_end_to_end`** — the **S-manager edge**, the
//!      one thing the other tests skip: they author device grants by
//!      `shared_b.tree.put` (B writing its own tree). This proves S — a *remote*
//!      peer holding only its seeded **manager grant** — can, under enforcement,
//!      (a) read B's observability subtrees and (b) author a *device's* grant
//!      over the wire via `system/capability` `configure`; the device then
//!      transfers a real file, and a never-authorized peer stays denied. This is
//!      the "is S locked out under enforcement?" risk from
//!      `DESIGN-ENFORCEMENT-CUTOVER.md`, proven headless.
//!
//! Native only; `sqlite`/`websocket`/`capability-handler` come from the
//! test-only dev-dependency.

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

/// **The reconnect contract** (the mechanism behind the cutover's one behavioral
/// requirement). A peer authorized *after* it already has a live connection does
/// NOT pick up the grant on that connection: B mints a peer's capability at
/// authenticate, and the client's connection pool reuses the existing (grantless)
/// connection — `get_or_connect` returns the pooled entry and `insert` is
/// insert-if-absent (`remote.rs`), so nothing re-dials or re-mints on its own.
///
/// The grant IS delivered in-band on the next authenticate — B re-mints when the
/// policy changed (`connection.rs` §9.1 R6-e). So the fix is NOT re-pairing and
/// NOT the capability `request` op (that self-mints/delegates; it does not pull
/// B's grant): the authorized peer must **drop its pooled connection and
/// reconnect**. This test pins both halves — stale reuse stays denied, drop +
/// reconnect confers — so the app's authorize flow keeps honoring it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorized_peer_needs_reconnect_to_adopt_grant() {
    let (shared_b, pid_b, addr, tmp) = build_backend(false).await; // ENFORCED
    std::fs::write(tmp.path().join(SEED_NAME), SEED_BODY).unwrap();

    // D connects while ungranted, then is authorized on B (direct put isolates
    // this from the configure path — that's covered by the S-manager test).
    let shared_d = build_client(21);
    dial(&shared_d, &addr).await;
    assert_ne!(
        probe_list(&shared_d, &pid_b).await,
        Some(200),
        "ungranted device is denied"
    );
    let d_hex = shared_d.identity_hash.to_hex();
    shared_b
        .tree
        .put(
            &format!("/{}/system/capability/policy/{}", pid_b, d_hex),
            file_transfer_policy(&pid_b, &d_hex),
        )
        .expect("author D's grant");

    // Still denied on the SAME pooled connection — the stale grantless cap is
    // reused (this is the behavior the app must not mistake for "not authorized").
    assert_ne!(
        probe_list(&shared_d, &pid_b).await,
        Some(200),
        "a peer authorized mid-connection stays denied on its existing pooled link"
    );

    // Drop the pooled connection + reconnect → B re-mints with the grant in-band.
    shared_d.remote.remove(&pid_b);
    dial(&shared_d, &addr).await;
    assert_eq!(
        probe_list(&shared_d, &pid_b).await,
        Some(200),
        "after drop+reconnect the peer adopts the grant — no re-pairing needed"
    );
}

// ---------------------------------------------------------------------------
// The S-manager edge (test 3): S authorizes a device over the wire.
// ---------------------------------------------------------------------------

/// The seeded share file the authorized device pulls.
const SEED_NAME: &str = "welcome.txt";
const SEED_BODY: &[u8] = b"hello from the backend share";

/// The **manager grant** as `src-tauri/src/manager_grant.rs` authors it, rebuilt
/// here (the app crate is bin-only, so its modules can't be imported — the same
/// reason `file_transfer_policy` is duplicated). The scopes are pinned to the
/// real seed by that module's own unit tests
/// (`read_grant_covers_session_and_capability_subtrees`,
/// `author_grant_targets_policy_prefix_via_configure`): `get` on B's
/// `system/peer/session/*` + `system/capability/*` via `system/tree`,
/// subscribe on the same via `system/subscription`, and `configure` on
/// `system/capability/policy/*` via `system/capability`.
fn manager_grant_policy(pid_b: &str, grantee: &str) -> Entity {
    let scoped = |p: &str| format!("/{}/{}", pid_b, p);
    let observe = vec![
        scoped("system/peer/session/*"),
        scoped("system/capability/*"),
    ];
    let grants = [
        GrantEntry {
            handlers: PathScope::new(vec!["system/tree".into()]),
            resources: PathScope::new(observe.clone()),
            operations: IdScope::new(vec!["get".into()]),
            peers: None,
            constraints: None,
            allowances: None,
        },
        GrantEntry {
            handlers: PathScope::new(vec!["system/subscription".into()]),
            resources: PathScope::new(observe),
            operations: IdScope::new(vec!["subscribe".into(), "unsubscribe".into()]),
            peers: None,
            constraints: None,
            allowances: None,
        },
        GrantEntry {
            handlers: PathScope::new(vec!["system/capability".into()]),
            resources: PathScope::new(vec![scoped("system/capability/policy/*")]),
            operations: IdScope::new(vec!["configure".into()]),
            peers: None,
            constraints: None,
            allowances: None,
        },
    ];
    let arr: Vec<_> = grants.iter().map(encode_grant_entry).collect();
    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
        (entity_ecf::text("grants"), entity_ecf::Value::Array(arr)),
        (entity_ecf::text("peer_pattern"), entity_ecf::text(grantee)),
    ]));
    Entity::new(POLICY_TYPE, data).unwrap()
}

/// Dispatch one op from `shared` against B, returning the full result (or `None`
/// when dispatch itself denied — a capability failure surfaced as an error).
/// The same pooled-connection execute the app drives (`make_execute_fn`).
async fn exec(
    shared: &Arc<PeerShared>,
    pid_b: &str,
    handler: &str,
    op: &str,
    params: Entity,
    resource_path: Option<String>,
) -> Option<entity_handler::HandlerResult> {
    let execute_fn = entity_peer::connection::make_execute_fn(
        shared.clone(),
        Some(shared.identity_hash),
        HashMap::new(),
        None,
        None,
    );
    let opts = ExecuteOptions {
        resource: resource_path.map(|p| ResourceTarget {
            targets: vec![p],
            exclude: vec![],
        }),
        ..Default::default()
    };
    execute_fn(
        format!("entity://{}/{}", pid_b, handler),
        op.to_string(),
        params,
        opts,
    )
    .await
    .ok()
}

/// The whole authorize gate as the *manager* drives it, under enforcement — the
/// path no other test covers (they all author device grants by B writing its own
/// tree). Proves: (a) S reads B's observability subtrees under the seeded manager
/// grant, (b) S authors a device grant over the wire via `configure`, (c) the
/// device then transfers a real file, and (d) a never-authorized peer stays
/// denied. This is `DESIGN-ENFORCEMENT-CUTOVER.md` Arm 1(b).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manager_authorizes_device_end_to_end() {
    let (shared_b, pid_b, addr, tmp) = build_backend(false).await; // ENFORCED
    std::fs::write(tmp.path().join(SEED_NAME), SEED_BODY).unwrap();

    // S, the manager. Seed its manager grant BEFORE it handshakes (exactly
    // `seed_manager_grant` at backend start), keyed by S's base58 id — the real
    // seed's key form (`base58_key_grant_confers` proves it self-heals to hex).
    let shared_s = build_client(10);
    let s_key = shared_s.keypair.peer_id().to_string();
    shared_b
        .tree
        .put(
            &format!("/{}/system/capability/policy/{}", pid_b, s_key),
            manager_grant_policy(&pid_b, &s_key),
        )
        .expect("seed manager grant");
    dial(&shared_s, &addr).await;

    // (a) S reads B's observability subtrees under the manager grant — the reads
    // the System Overview + backend-auth refresh depend on. Not locked out.
    let sessions = exec(
        &shared_s,
        &pid_b,
        "system/tree",
        "get",
        empty_params(),
        Some(format!("/{}/system/peer/session/", pid_b)),
    )
    .await;
    assert_eq!(
        sessions.map(|r| r.status),
        Some(200),
        "manager S must read B's session subtree under its grant (not locked out)"
    );
    let policy = exec(
        &shared_s,
        &pid_b,
        "system/tree",
        "get",
        empty_params(),
        Some(format!("/{}/system/capability/policy/", pid_b)),
    )
    .await;
    assert_eq!(
        policy.map(|r| r.status),
        Some(200),
        "manager S must read B's capability-policy subtree under its grant"
    );

    // Device D connects first and is DENIED (the pending state), then S grants it.
    let shared_d = build_client(11);
    dial(&shared_d, &addr).await;
    assert_ne!(
        probe_list(&shared_d, &pid_b).await,
        Some(200),
        "an un-authorized device must be denied the share before S grants it"
    );

    // (b) S authorizes D over the wire via `configure` — the EXACT call the app
    // makes (`handle_authorize_peer`): resource None, body = the file-transfer
    // grant keyed by D's identity-hash hex (the form B reports D as in its
    // session listing, i.e. what the app authorizes by).
    let d_key = shared_d.identity_hash.to_hex();
    let configured = exec(
        &shared_s,
        &pid_b,
        "system/capability",
        "configure",
        file_transfer_policy(&pid_b, &d_key),
        None,
    )
    .await;
    assert_eq!(
        configured.map(|r| r.status),
        Some(200),
        "S's manager grant must confer authoring a device grant via `configure`"
    );

    // The grant applies on D's NEXT handshake (design §3 Step 4: fresh reconnect)
    // — B mints a peer's capability from the policy table AT authenticate, and a
    // pooled re-dial reuses the already-authenticated (grantless) cap. A device
    // must therefore FULLY reconnect after being authorized; a new client of the
    // SAME identity (seed 11) models exactly that fresh connection. **Cutover
    // consequence:** the app's authorize flow must force the device to reconnect,
    // not merely re-handshake on the pooled link (DESIGN-ENFORCEMENT-CUTOVER §3).
    let shared_d = build_client(11);
    dial(&shared_d, &addr).await;

    // (c) D transfers a real file: list shows it, read returns its bytes.
    let list = exec(
        &shared_d,
        &pid_b,
        "local/files",
        "list",
        empty_params(),
        Some(format!("/{}/local/files/shared/", pid_b)),
    )
    .await
    .expect("list dispatches");
    assert_eq!(list.status, 200, "authorized device's list must succeed");
    assert!(
        String::from_utf8_lossy(&list.result.data).contains(SEED_NAME),
        "list must show the seeded file"
    );
    let read = exec(
        &shared_d,
        &pid_b,
        "local/files",
        "read",
        empty_params(),
        Some(format!("/{}/local/files/shared/{}", pid_b, SEED_NAME)),
    )
    .await
    .expect("read dispatches");
    assert_eq!(read.status, 200, "authorized device's pull (read) must succeed");
    assert!(!read.included.is_empty(), "pull must carry the file content");

    // (d) A peer S never authorized stays denied even now.
    let shared_e = build_client(12);
    dial(&shared_e, &addr).await;
    assert_ne!(
        probe_list(&shared_e, &pid_b).await,
        Some(200),
        "a never-authorized peer must remain denied even after S authorized D"
    );
}

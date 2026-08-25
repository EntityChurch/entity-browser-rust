//! End-to-end proof that the **File Transfer window's actual flow works** —
//! connect → list → pull(read) → upload(write) — against a real backend peer
//! over a real WebSocket, exactly as `src/dom/file_transfer.rs` drives it
//! (`Action::Execute` list, `Action::DownloadFile` read, `Action::UploadFile`
//! write, all as `entity://{target}/local/files` ops). This is the thing the
//! unit tests could not prove: that a file actually moves.
//!
//! Two postures, both asserted:
//!   1. `debug_open_grants = true` — what the shipped desktop backend runs today
//!      (`src-tauri/src/lib.rs`). Every op must succeed (kernel wildcard).
//!   2. `debug_open_grants = false` + an authored file-transfer policy grant —
//!      the post-step-7 gated world: list/read succeed under the grant.
//!
//! Native only; `sqlite`/`websocket`/`capability-handler` come from the
//! test-only dev-dependency (see `AGENTS.md` §Mechanism proof). Mirrors the
//! harness in `tests/authorize_gate.rs` + `tests/local_files_write_persistence.rs`.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use entity_capability::{encode_grant_entry, GrantEntry, IdScope, PathScope, ResourceTarget};
use entity_crypto::Keypair;
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_peer::local_files::RootConfigData;
use entity_peer::transport::{Connector, WebSocketConnector, WebSocketListener};
use entity_peer::{PeerBuilder, PeerConfig, PeerShared};

const POLICY_TYPE: &str = "system/capability/policy-entry";

/// The seeded file every fresh pairing has to transfer (`SHARE_PREFIX` +
/// `DEFAULT_FILENAME`, `src/views/file_transfer/model.rs`).
const SEED_NAME: &str = "welcome.txt";
const SEED_BODY: &[u8] = b"hello from the backend share";

fn empty_params() -> Entity {
    Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap()
}

/// The exact upload body the app sends (`local/files/write-request`, `bytes`).
fn write_params(bytes: &[u8]) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "bytes" => entity_ecf::Value::Bytes(bytes.to_vec())
    });
    Entity::new("local/files/write-request", data).unwrap()
}

/// A file-transfer policy grant (`list`+`read` on B's share) keyed by `grantee`.
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

/// Build backend B: SQLite tree + writable `shared` root (a real WS listener),
/// exactly the shape the Tauri desktop backend is built in. Seeds `welcome.txt`
/// into the share so a `list` has something to show. Returns
/// `(shared_b, pid_b, ws_addr, tmp_root)` — hold the tmp dirs for the test.
async fn build_backend(
    debug_open: bool,
) -> (Arc<PeerShared>, String, String, tempfile::TempDir, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let kp_b = Keypair::from_seed([9u8; 32]);
    let pid_b = kp_b.peer_id().to_string();
    let peer_b = PeerBuilder::new()
        .keypair(kp_b)
        .config(PeerConfig { debug_open_grants: debug_open, ..PeerConfig::default() })
        .sqlite(db_dir.path().join("backend.sqlite"))
        .expect("open sqlite store")
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

    // Seed the share (direct FS write is picked up by `list` — see the
    // extension's own `handler_ops.rs` list test).
    std::fs::write(tmp.path().join(SEED_NAME), SEED_BODY).unwrap();

    let shared_b_run = shared_b.clone();
    let listener = WebSocketListener::bind("127.0.0.1:0").await.expect("ws bind");
    let addr = format!("ws://{}", listener.socket_addr());
    tokio::spawn(async move {
        let _ = entity_peer::server::run(listener, shared_b_run).await;
    });
    (shared_b, pid_b, addr, tmp, db_dir)
}

/// Client A, `debug_open_grants` on (governs what A may initiate; B enforces).
fn build_client(seed: u8) -> Arc<PeerShared> {
    let peer_a = PeerBuilder::new()
        .keypair(Keypair::from_seed([seed; 32]))
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>)
        .build()
        .expect("peer A builds");
    let shared_a = peer_a.shared();
    peer_a.start_engines(&shared_a);
    shared_a
}

/// Dial + handshake A→B against A's persistent shared, pooling the connection
/// (exactly `PeerContext::connect_to` / what the app does on ConnectPeer).
async fn dial(shared_a: &Arc<PeerShared>, addr: &str) -> String {
    let conn = shared_a.connector.connect(addr).await.expect("A dials B");
    let remote = entity_peer::remote::perform_connect_with_dispatch(
        conn,
        &shared_a.keypair,
        shared_a.config.home_hash_format,
        Some(shared_a.clone()),
        // §4.4: dial-by-address test harness — no §3 rendezvous key, so no
        // reciprocal grant (asymmetric establishment, §6.6).
        false,
    )
    .await
    .expect("A handshakes B");
    let pid = remote.remote_peer_id.clone();
    shared_a.remote.insert(&pid, remote);
    pid
}

/// An execute_fn bound to A's persistent shared — the pooled connection dispatch
/// reads from (the single-`shared` discipline the SDK/app enforce). Returned as
/// the `Arc<dyn Fn…>` `make_execute_fn` yields (directly callable).
fn execute_fn_for(
    shared_a: &Arc<PeerShared>,
) -> Arc<
    dyn Fn(
            String,
            String,
            Entity,
            ExecuteOptions,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<entity_handler::HandlerResult, entity_handler::HandlerError>,
                    > + Send,
            >,
        > + Send
        + Sync,
> {
    entity_peer::connection::make_execute_fn(
        shared_a.clone(),
        Some(shared_a.identity_hash),
        HashMap::new(),
        None,
        None,
    )
}

fn resource(path: &str) -> ExecuteOptions {
    ExecuteOptions {
        resource: Some(ResourceTarget { targets: vec![path.to_string()], exclude: vec![] }),
        ..Default::default()
    }
}

/// POSTURE 1 — the shipped desktop backend (`debug_open_grants = true`).
/// The full window flow must work end to end: list shows the seeded file, a
/// pull returns its content, and an upload lands on B's disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_transfer_round_trip_debug_open() {
    let (_shared_b, pid_b, addr, tmp, _db) = build_backend(true).await;
    let shared_a = build_client(1);
    dial(&shared_a, &addr).await;
    let exec = execute_fn_for(&shared_a);

    // --- LIST (the "List shared files" button) ---
    let list = exec(
        format!("entity://{}/local/files", pid_b),
        "list".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/", pid_b)),
    )
    .await
    .expect("list dispatches");
    assert_eq!(list.status, 200, "list must succeed with debug_open_grants on");
    let listing = String::from_utf8_lossy(&list.result.data);
    assert!(
        listing.contains(SEED_NAME),
        "list must show the seeded {SEED_NAME}; got: {listing:?}"
    );

    // --- READ / PULL (the "Pull file" button → Action::DownloadFile) ---
    let read = exec(
        format!("entity://{}/local/files", pid_b),
        "read".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/{}", pid_b, SEED_NAME)),
    )
    .await
    .expect("read dispatches");
    assert_eq!(read.status, 200, "pull (read) must return the file, got {}", read.status);
    assert!(
        !read.result.entity_type.is_empty(),
        "read must return a file entity"
    );
    assert!(
        !read.included.is_empty(),
        "read must carry the file content (blob/chunks) in `included`; got {} included",
        read.included.len()
    );

    // --- WRITE / UPLOAD (the "Upload a file" picker → Action::UploadFile) ---
    let uploaded = b"pushed from the client";
    let write = exec(
        format!("entity://{}/local/files", pid_b),
        "write".into(),
        write_params(uploaded),
        resource(&format!("/{}/local/files/shared/pushed.txt", pid_b)),
    )
    .await
    .expect("write dispatches");
    assert_eq!(write.status, 200, "upload (write) must succeed, got {}", write.status);

    // The file must actually be on B's disk — and stay (the Tori self-delete
    // regression guard: give the watcher/reverse-writer a beat to churn).
    let landed = tmp.path().join("pushed.txt");
    assert!(landed.exists(), "uploaded file must exist on the backend immediately");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(landed.exists(), "uploaded file must survive the watcher debounce");
    assert_eq!(std::fs::read(&landed).unwrap(), uploaded, "on-disk bytes match the upload");
}

/// The TREE data source: a nested share must list one directory at a time, and
/// the real `list` output must carry the exact fields the app's `decode_listing`
/// keys on (`entity_path`, `entry_type`, `directory`). This proves the tree
/// browser's per-directory lazy model against real handler output (the decode
/// itself is unit-tested against this same shape in `file_transfer/model.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_transfer_tree_listing_is_nested_and_shaped() {
    let (_shared_b, pid_b, addr, tmp, _db) = build_backend(true).await;
    // Seed a sub-directory with its own file alongside the root welcome.txt.
    std::fs::create_dir(tmp.path().join("docs")).unwrap();
    std::fs::write(tmp.path().join("docs/readme.md"), b"# readme").unwrap();

    let shared_a = build_client(4);
    dial(&shared_a, &addr).await;
    let exec = execute_fn_for(&shared_a);

    // Root listing: the file, the sub-directory, and the field names + dir type
    // the app decodes must all be present in the real handler output.
    let root = exec(
        format!("entity://{}/local/files", pid_b),
        "list".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/", pid_b)),
    )
    .await
    .expect("root list dispatches");
    assert_eq!(root.status, 200);
    let body = String::from_utf8_lossy(&root.result.data);
    for needle in ["welcome.txt", "docs", "entity_path", "entry_type", "directory"] {
        assert!(body.contains(needle), "root listing must contain {needle:?}; got {body:?}");
    }

    // Listing the sub-directory returns *its* child — the lazy per-dir expand.
    let sub = exec(
        format!("entity://{}/local/files", pid_b),
        "list".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/docs/", pid_b)),
    )
    .await
    .expect("subdir list dispatches");
    assert_eq!(sub.status, 200, "listing a sub-directory must succeed");
    assert!(
        String::from_utf8_lossy(&sub.result.data).contains("readme.md"),
        "sub-directory listing must show its child file"
    );
}

/// POSTURE 2 — the gated world (`debug_open_grants = false`) with an authored
/// file-transfer grant. The reads the window depends on must succeed under the
/// real policy grant (proving the gate lets an authorized peer transfer). The
/// grant is authored BEFORE A's first handshake (the §2 fresh-handshake fact).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_transfer_reads_work_under_authored_grant() {
    let (shared_b, pid_b, addr, _tmp, _db) = build_backend(false).await;
    let shared_a = build_client(2);

    // Author A's grant (hex-keyed, the canonical form) before the handshake.
    let a_hex = shared_a.identity_hash.to_hex();
    shared_b
        .tree
        .put(
            &format!("/{}/system/capability/policy/{}", pid_b, a_hex),
            file_transfer_policy(&pid_b, &a_hex),
        )
        .expect("author policy grant");

    dial(&shared_a, &addr).await;
    let exec = execute_fn_for(&shared_a);

    let list = exec(
        format!("entity://{}/local/files", pid_b),
        "list".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/", pid_b)),
    )
    .await
    .expect("list dispatches");
    assert_eq!(list.status, 200, "authorized peer's list must succeed under the grant");
    assert!(String::from_utf8_lossy(&list.result.data).contains(SEED_NAME));

    let read = exec(
        format!("entity://{}/local/files", pid_b),
        "read".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/{}", pid_b, SEED_NAME)),
    )
    .await
    .expect("read dispatches");
    assert_eq!(read.status, 200, "authorized peer's pull must succeed under the grant");
    assert!(!read.included.is_empty(), "pull returns content");
}

/// The negative that makes the gate meaningful: `debug_open_grants = false`,
/// NO grant → the window's list/read are DENIED (not 200). This is the state
/// the honest "⛔ Not authorized" affordance is meant to reflect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_transfer_denied_without_grant() {
    let (_shared_b, pid_b, addr, _tmp, _db) = build_backend(false).await;
    let shared_a = build_client(3);
    dial(&shared_a, &addr).await;
    let exec = execute_fn_for(&shared_a);

    let status = exec(
        format!("entity://{}/local/files", pid_b),
        "list".into(),
        empty_params(),
        resource(&format!("/{}/local/files/shared/", pid_b)),
    )
    .await
    .map(|r| r.status)
    .ok();
    assert_ne!(status, Some(200), "an ungranted peer must NOT be able to list the share");
}
